//! `tnls-rendezvous-gui` — a native egui front-end for `tnls rendezvous`: send a file
//! over a capability-scoped BitTorrent tunnel, or receive one from a link.
//!
//! The GUI drives the `tnls` libraries in-process (`tnls_tunnel::session::open` for send,
//! `tnls_rendezvous::{get, bittorrent}` for receive); it adds no tunnel/crypto logic of its
//! own — enforcement stays at the agent. See `app.rs`.

mod app;
mod driver;

fn main() -> eframe::Result<()> {
    // rustls 0.23 needs a process-wide provider before any wss:// dial (the relay handshake
    // on send, the viewer socket + magnet retrieval on receive).
    let _ = rustls::crypto::ring::default_provider().install_default();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 480.0])
            .with_min_inner_size([560.0, 360.0])
            .with_title("tnls · rendezvous"),
        ..Default::default()
    };
    eframe::run_native(
        "tnls-rendezvous-gui",
        options,
        Box::new(|_cc| Ok(Box::new(app::App::new()))),
    )
}
