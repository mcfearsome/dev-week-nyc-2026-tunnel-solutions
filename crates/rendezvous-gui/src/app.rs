//! The egui/eframe window: a Send pane (open a scoped tunnel that seeds + serves a file)
//! and a Receive pane (fetch a shared file from a link), both driven **in-process** against
//! the `tnls` libraries — no CLI shelling, no stdout scraping.
//!
//! - Receive calls `tnls_rendezvous::get::retrieve_magnet` + `bittorrent::fetch` and polls
//!   the live `FetchHandle` for progress + speed.
//! - Send calls `tnls_tunnel::session::open` directly: the tunnel agent runs in this process,
//!   hands us the link via a callback, and revokes when we notify its `shutdown`. (The agent
//!   still spawns the `tnls-rendezvous` binary as the scoped MCP child that seeds the file —
//!   that child *is* the architecture, not a CLI we parse.)
//!
//! A background multi-thread tokio runtime owns the async work; the egui frame loop reads
//! shared state behind a mutex and the workers call `request_repaint` as state changes.

use crate::driver;
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone)]
enum ShareState {
    Idle,
    Starting,
    Active { link: String },
    Stopped,
    Failed { message: String },
}

#[derive(Clone)]
enum RecvState {
    Idle,
    Resolving,
    Fetching {
        pct: u8,
        downloaded: u64,
        total: u64,
        down_bps: f64,
    },
    Done {
        path: Option<String>,
    },
    Failed {
        message: String,
    },
}

pub struct App {
    rt: Option<tokio::runtime::Runtime>,
    rendezvous_bin: Option<PathBuf>,
    relay: String,

    share_path: String,
    ttl: String,
    share_state: Arc<Mutex<ShareState>>,
    share_shutdown: Option<Arc<Notify>>,

    link: String,
    out_dir: String,
    recv_state: Arc<Mutex<RecvState>>,
    recv_cancel: Option<Arc<AtomicBool>>,
}

impl App {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        Self {
            rt: Some(rt),
            rendezvous_bin: driver::resolve_rendezvous(),
            relay: "wss://tunnel.locker".to_string(),
            share_path: String::new(),
            ttl: "30m".to_string(),
            share_state: Arc::new(Mutex::new(ShareState::Idle)),
            share_shutdown: None,
            link: String::new(),
            out_dir: "./tnls-downloads".to_string(),
            recv_state: Arc::new(Mutex::new(RecvState::Idle)),
            recv_cancel: None,
        }
    }

    fn set_share(&self, s: ShareState) {
        *self.share_state.lock().unwrap() = s;
    }
    fn set_recv(&self, s: RecvState) {
        *self.recv_state.lock().unwrap() = s;
    }

    fn start_share(&mut self, ctx: &egui::Context) {
        let Some(bin) = self.rendezvous_bin.clone() else {
            self.set_share(ShareState::Failed {
                message:
                    "`tnls-rendezvous` not found — build the workspace or set $TNLS_RENDEZVOUS_BIN"
                        .into(),
            });
            return;
        };
        let file = self.share_path.trim().to_string();
        if file.is_empty() {
            self.set_share(ShareState::Failed {
                message: "pick a file to share (drag one in, or type a path)".into(),
            });
            return;
        }
        let ttl = match tnls_core::parse_ttl(self.ttl.trim()) {
            Ok(d) => d,
            Err(e) => {
                self.set_share(ShareState::Failed {
                    message: format!("bad TTL: {e}"),
                });
                return;
            }
        };

        self.set_share(ShareState::Starting);
        let shutdown = Arc::new(Notify::new());
        self.share_shutdown = Some(shutdown.clone());

        let relay = self.relay.trim().to_string();
        let server = bin.to_string_lossy().into_owned();
        let state_for_link = self.share_state.clone();
        let state_for_end = self.share_state.clone();
        let ctx_link = ctx.clone();
        let ctx_end = ctx.clone();

        self.rt.as_ref().unwrap().spawn(async move {
            let on_link = Box::new(move |link: String| {
                *state_for_link.lock().unwrap() = ShareState::Active { link };
                ctx_link.request_repaint();
            });
            let res = tnls_tunnel::session::open(tnls_tunnel::session::OpenArgs {
                server,
                server_args: vec!["share".to_string(), file],
                ttl,
                // Matches the rendezvous `describe` manifest for the tunneled `share` command.
                scope: vec!["list_shares".to_string(), "request_file".to_string()],
                relay: relay.clone(),
                env: vec![("TNLS_RELAY".to_string(), relay)],
                quiet: true,
                on_link: Some(on_link),
                shutdown: Some(shutdown),
            })
            .await;

            let mut s = state_for_end.lock().unwrap();
            match res {
                // Live → revoked/expired: the link is dead. Don't clobber a Failed set earlier.
                Ok(()) => {
                    if !matches!(*s, ShareState::Failed { .. }) {
                        *s = ShareState::Stopped;
                    }
                }
                Err(e) => {
                    *s = ShareState::Failed {
                        message: format!("{e:#}"),
                    }
                }
            }
            ctx_end.request_repaint();
        });
    }

    fn stop_share(&mut self) {
        if let Some(sd) = self.share_shutdown.take() {
            sd.notify_one(); // bridges into the agent's teardown (kills seeder, drops pidfile)
        }
    }

    fn start_get(&mut self, ctx: &egui::Context) {
        let link = self.link.trim().to_string();
        if link.is_empty() {
            self.set_recv(RecvState::Failed {
                message: "paste a tunnel link to fetch".into(),
            });
            return;
        }
        let out = PathBuf::from(self.out_dir.trim());

        self.set_recv(RecvState::Resolving);
        let cancel = Arc::new(AtomicBool::new(false));
        self.recv_cancel = Some(cancel.clone());
        let state = self.recv_state.clone();
        let ctx = ctx.clone();

        self.rt.as_ref().unwrap().spawn(async move {
            use tnls_rendezvous::bittorrent::{self, NetOpts};
            use tnls_rendezvous::get;

            let fail = |state: &Arc<Mutex<RecvState>>, ctx: &egui::Context, msg: String| {
                *state.lock().unwrap() = RecvState::Failed { message: msg };
                ctx.request_repaint();
            };

            // Control plane: pull the magnet (+ advertised peers) through the scoped tunnel.
            let rf = match get::retrieve_magnet(&link).await {
                Ok(rf) => rf,
                Err(e) => return fail(&state, &ctx, format!("{e:#}")),
            };
            if let Err(e) = std::fs::create_dir_all(&out) {
                return fail(
                    &state,
                    &ctx,
                    format!("cannot create {}: {e}", out.display()),
                );
            }

            // Test hook mirrors the CLI: TNLS_DISABLE_DHT makes the fetch hermetic (loopback).
            let hermetic = std::env::var_os("TNLS_DISABLE_DHT").is_some_and(|v| !v.is_empty());
            let dl = match bittorrent::fetch(
                &rf.magnet,
                &out,
                NetOpts {
                    disable_dht: hermetic,
                    listen_port: None,
                    enable_upnp: !hermetic,
                    initial_peers: rf.peers,
                },
            )
            .await
            {
                Ok(dl) => dl,
                Err(e) => return fail(&state, &ctx, format!("{e:#}")),
            };

            // Data plane: poll the live torrent for progress until done or cancelled.
            loop {
                if cancel.load(Ordering::SeqCst) {
                    *state.lock().unwrap() = RecvState::Idle;
                    ctx.request_repaint();
                    break; // dropping `dl` here stops the session
                }
                let p = dl.progress();
                if p.finished {
                    *state.lock().unwrap() = RecvState::Done {
                        path: dl.output_path().map(|p| p.display().to_string()),
                    };
                    ctx.request_repaint();
                    break;
                }
                let pct = (p.downloaded.saturating_mul(100))
                    .checked_div(p.total)
                    .unwrap_or(0) as u8;
                *state.lock().unwrap() = RecvState::Fetching {
                    pct,
                    downloaded: p.downloaded,
                    total: p.total,
                    down_bps: p.down_speed_bps,
                };
                ctx.request_repaint();
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        });
    }

    fn cancel_get(&mut self) {
        if let Some(c) = self.recv_cancel.take() {
            c.store(true, Ordering::SeqCst);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Drag-and-drop a file onto the window to populate the Send path.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.into_iter().find_map(|f| f.path) {
            self.share_path = path.to_string_lossy().into_owned();
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.heading("tnls · rendezvous");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.relay)
                            .desired_width(220.0)
                            .hint_text("relay"),
                    );
                    ui.label("relay:");
                });
            });
            if self.rendezvous_bin.is_none() {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd9, 0x53, 0x4f),
                    "⚠ `tnls-rendezvous` not found next to this binary or on $PATH — set $TNLS_RENDEZVOUS_BIN.",
                );
            }
            ui.separator();
            ui.add_space(4.0);

            ui.columns(2, |cols| {
                self.share_pane(&mut cols[0], &ctx);
                self.recv_pane(&mut cols[1], &ctx);
            });
        });

        // Keep repainting while async work is in flight (workers also request repaints).
        let busy = matches!(
            &*self.share_state.lock().unwrap(),
            ShareState::Starting | ShareState::Active { .. }
        ) || matches!(
            &*self.recv_state.lock().unwrap(),
            RecvState::Resolving | RecvState::Fetching { .. }
        );
        if busy {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
}

impl App {
    fn share_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Send a file");
        ui.label("Seed a file and open a capability-scoped link. The link dies on TTL or stop.");
        ui.add_space(8.0);

        let state = self.share_state.lock().unwrap().clone();
        let active = matches!(state, ShareState::Active { .. } | ShareState::Starting);

        ui.add_enabled_ui(!active, |ui| {
            ui.horizontal(|ui| {
                ui.label("File:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.share_path)
                        .desired_width(f32::INFINITY)
                        .hint_text("drag a file here, or type a path"),
                );
            });
            ui.horizontal(|ui| {
                ui.label("TTL:");
                ui.add(egui::TextEdit::singleline(&mut self.ttl).desired_width(80.0));
                if ui.button("Share").clicked() {
                    self.start_share(ctx);
                }
            });
        });

        ui.add_space(10.0);
        match state {
            ShareState::Idle => {
                ui.weak("Idle — nothing shared yet.");
            }
            ShareState::Starting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Seeding and opening the tunnel…");
                });
            }
            ShareState::Active { link } => {
                ui.label("Live link — hand this to a teammate:");
                ui.add(
                    egui::TextEdit::multiline(&mut link.clone())
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                ui.horizontal(|ui| {
                    if ui.button("Copy link").clicked() {
                        ui.ctx().copy_text(link.clone());
                    }
                    if ui.button("Stop sharing").clicked() {
                        self.stop_share();
                    }
                });
                ui.add_space(4.0);
                ui.weak(
                    "Seeding over BitTorrent — bytes go peer-to-peer, never through the relay.",
                );
            }
            ShareState::Stopped => {
                ui.colored_label(egui::Color32::GRAY, "Stopped — the link is dead.");
            }
            ShareState::Failed { message } => {
                ui.colored_label(egui::Color32::from_rgb(0xd9, 0x53, 0x4f), message);
            }
        }
    }

    fn recv_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Receive a file");
        ui.label("Paste a tunnel link to fetch the shared file over BitTorrent.");
        ui.add_space(8.0);

        let state = self.recv_state.lock().unwrap().clone();
        let busy = matches!(state, RecvState::Resolving | RecvState::Fetching { .. });

        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                ui.label("Link:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.link)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://…/t/<id>#<token>"),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Into:");
                ui.add(egui::TextEdit::singleline(&mut self.out_dir).desired_width(f32::INFINITY));
            });
            if ui.button("Fetch").clicked() {
                self.start_get(ctx);
            }
        });

        ui.add_space(10.0);
        match state {
            RecvState::Idle => {
                ui.weak("Idle — nothing fetched yet.");
            }
            RecvState::Resolving => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Resolving the magnet through the tunnel…");
                });
            }
            RecvState::Fetching {
                pct,
                downloaded,
                total,
                down_bps,
            } => {
                let frac = if total > 0 {
                    downloaded as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(frac).text(format!("{pct}%")));
                ui.weak(format!(
                    "{} / {}  ·  ↓ {}/s",
                    human_bytes(downloaded),
                    human_bytes(total),
                    human_bytes(down_bps as u64),
                ));
                if ui.button("Cancel").clicked() {
                    self.cancel_get();
                }
            }
            RecvState::Done { path } => {
                ui.colored_label(
                    egui::Color32::from_rgb(0x3c, 0xa8, 0x4b),
                    "✓ Download complete.",
                );
                if let Some(p) = path {
                    ui.add(
                        egui::TextEdit::singleline(&mut p.clone())
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                }
            }
            RecvState::Failed { message } => {
                ui.colored_label(egui::Color32::from_rgb(0xd9, 0x53, 0x4f), message);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Revoke the share / stop the download, then give the agent teardown a bounded window.
        self.stop_share();
        self.cancel_get();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_secs(4));
        }
    }
}

/// Compact human-readable byte count (binary units).
fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} {}", U[0])
    } else {
        format!("{v:.1} {}", U[i])
    }
}
