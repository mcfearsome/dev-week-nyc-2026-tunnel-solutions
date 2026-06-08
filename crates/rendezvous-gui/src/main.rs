//! `tnls-rendezvous-gui` — a native egui front-end for `tnls rendezvous`: send a file
//! over a capability-scoped BitTorrent tunnel, or receive one from a link.
//!
//! It is a thin driver over the `tnls` host CLI (see `app.rs` for why), so the GUI
//! adds no tunnel/crypto logic of its own — enforcement stays at the agent.

mod app;
mod driver;

fn main() -> eframe::Result<()> {
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
