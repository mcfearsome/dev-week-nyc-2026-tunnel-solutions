//! The egui/eframe window: a Send pane (seed a file + open a scoped tunnel) and a
//! Receive pane (fetch a shared file from a link), each driving a `tnls` subprocess.
//!
//! Why subprocesses rather than calling the libraries in-process: all tunnel
//! enforcement (token, scope, TTL) lives in the `tnls` host + agent, and that is the
//! security boundary we must not duplicate. The GUI is a thin front-end — it spawns
//! `tnls rendezvous share|get`, reads the lines they print, and SIGTERMs the child to
//! revoke (which runs the host's real teardown: kill the seeder, drop the pidfile).

use crate::driver;
use eframe::egui;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

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
    Fetching {
        pct: u8,
        downloaded: u64,
        total: u64,
    },
    Done {
        path: Option<String>,
    },
    Failed {
        message: String,
    },
}

/// A running child we can revoke/cancel by signalling its pid.
struct Job {
    pid: u32,
    cancel: Arc<AtomicBool>,
}

impl Job {
    fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        // SIGTERM (not SIGKILL) so the host runs its graceful teardown.
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(self.pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
}

pub struct App {
    tnls: Option<PathBuf>,
    relay: String,

    share_path: String,
    ttl: String,
    share_state: Arc<Mutex<ShareState>>,
    share_job: Option<Job>,

    link: String,
    out_dir: String,
    recv_state: Arc<Mutex<RecvState>>,
    recv_job: Option<Job>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tnls: driver::resolve_tnls(),
            relay: "wss://tunnel.locker".to_string(),
            share_path: String::new(),
            ttl: "30m".to_string(),
            share_state: Arc::new(Mutex::new(ShareState::Idle)),
            share_job: None,
            link: String::new(),
            out_dir: "./tnls-downloads".to_string(),
            recv_state: Arc::new(Mutex::new(RecvState::Idle)),
            recv_job: None,
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    fn start_share(&mut self, ctx: &egui::Context) {
        let Some(tnls) = self.tnls.clone() else {
            *self.share_state.lock().unwrap() = ShareState::Failed {
                message: "`tnls` binary not found — build the workspace or set $TNLS_BIN".into(),
            };
            return;
        };
        let file = self.share_path.trim().to_string();
        if file.is_empty() {
            *self.share_state.lock().unwrap() = ShareState::Failed {
                message: "pick a file to share (drag one in, or type a path)".into(),
            };
            return;
        }

        *self.share_state.lock().unwrap() = ShareState::Starting;
        let mut child = match Command::new(&tnls)
            .args(["--relay", &self.relay, "--ttl", &self.ttl])
            .args(["rendezvous", "share", &file])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                *self.share_state.lock().unwrap() = ShareState::Failed {
                    message: format!("could not launch tnls: {e}"),
                };
                return;
            }
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.share_job = Some(Job {
            pid: child.id(),
            cancel: cancel.clone(),
        });

        let errbuf = collect_stderr(child.stderr.take());
        let stdout = child.stdout.take().expect("piped stdout");
        let state = self.share_state.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let mut got_link = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(link) = driver::extract_link(&line) {
                    got_link = true;
                    *state.lock().unwrap() = ShareState::Active { link };
                    ctx.request_repaint();
                }
            }
            // stdout closed → the host process is exiting (revoked, or TTL lapsed).
            let _ = child.wait();
            let mut s = state.lock().unwrap();
            *s = if got_link {
                ShareState::Stopped
            } else {
                ShareState::Failed {
                    message: last_line(&errbuf)
                        .unwrap_or_else(|| "tnls exited before opening a tunnel".into()),
                }
            };
            ctx.request_repaint();
        });
    }

    fn stop_share(&mut self) {
        if let Some(job) = self.share_job.take() {
            job.stop();
        }
    }

    fn start_get(&mut self, ctx: &egui::Context) {
        let Some(tnls) = self.tnls.clone() else {
            *self.recv_state.lock().unwrap() = RecvState::Failed {
                message: "`tnls` binary not found — build the workspace or set $TNLS_BIN".into(),
            };
            return;
        };
        let link = self.link.trim().to_string();
        if link.is_empty() {
            *self.recv_state.lock().unwrap() = RecvState::Failed {
                message: "paste a tunnel link to fetch".into(),
            };
            return;
        }
        let out = self.out_dir.trim().to_string();

        *self.recv_state.lock().unwrap() = RecvState::Fetching {
            pct: 0,
            downloaded: 0,
            total: 0,
        };
        let mut child = match Command::new(&tnls)
            .args(["rendezvous", "get", &link, "--out", &out])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                *self.recv_state.lock().unwrap() = RecvState::Failed {
                    message: format!("could not launch tnls: {e}"),
                };
                return;
            }
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.recv_job = Some(Job {
            pid: child.id(),
            cancel: cancel.clone(),
        });

        let errbuf = collect_stderr(child.stderr.take());
        let stdout = child.stdout.take().expect("piped stdout");
        let state = self.recv_state.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(p) = driver::parse_progress(&line) {
                    *state.lock().unwrap() = RecvState::Fetching {
                        pct: p.pct,
                        downloaded: p.downloaded,
                        total: p.total,
                    };
                    ctx.request_repaint();
                } else if let Some(path) = driver::parse_done(&line) {
                    *state.lock().unwrap() = RecvState::Done { path };
                    ctx.request_repaint();
                }
            }
            let status = child.wait();
            let mut s = state.lock().unwrap();
            if matches!(&*s, RecvState::Done { .. }) {
                // keep the completed state
            } else if cancel.load(Ordering::SeqCst) {
                *s = RecvState::Idle;
            } else {
                *s = RecvState::Failed {
                    message: last_line(&errbuf)
                        .unwrap_or_else(|| format!("download ended unexpectedly ({status:?})")),
                };
            }
            ctx.request_repaint();
        });
    }

    fn cancel_get(&mut self) {
        if let Some(job) = self.recv_job.take() {
            job.stop();
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
            if self.tnls.is_none() {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd9, 0x53, 0x4f),
                    "⚠ `tnls` not found next to this binary or on $PATH — set $TNLS_BIN.",
                );
            }
            ui.separator();
            ui.add_space(4.0);

            ui.columns(2, |cols| {
                self.share_pane(&mut cols[0], &ctx);
                self.recv_pane(&mut cols[1], &ctx);
            });
        });

        // Keep repainting while a subprocess is producing output.
        let busy = matches!(
            &*self.share_state.lock().unwrap(),
            ShareState::Starting | ShareState::Active { .. }
        ) || matches!(
            &*self.recv_state.lock().unwrap(),
            RecvState::Fetching { .. }
        );
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
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
                if let Some(id) = driver::tunnel_id_from_link(&link) {
                    ui.label(format!(
                        "Live link (tunnel {id}) — hand this to a teammate:"
                    ));
                } else {
                    ui.label("Live link — hand this to a teammate:");
                }
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
        let fetching = matches!(state, RecvState::Fetching { .. });

        ui.add_enabled_ui(!fetching, |ui| {
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
            RecvState::Fetching {
                pct,
                downloaded,
                total,
            } => {
                let frac = if total > 0 {
                    downloaded as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(frac).text(format!("{pct}%")));
                ui.weak(format!("{} / {} bytes", downloaded, total));
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
        // Don't orphan a live tunnel / download when the window closes.
        self.stop_share();
        self.cancel_get();
    }
}

/// Drain a child's stderr into a shared buffer on a background thread.
fn collect_stderr(stderr: Option<std::process::ChildStderr>) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = stderr {
        let buf = buf.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut b = buf.lock().unwrap();
                b.push_str(&line);
                b.push('\n');
            }
        });
    }
    buf
}

/// The last non-empty line captured so far — the most useful bit of an error.
fn last_line(buf: &Arc<Mutex<String>>) -> Option<String> {
    let b = buf.lock().unwrap();
    b.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
}
