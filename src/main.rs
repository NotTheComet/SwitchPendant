mod app;
mod input;
mod transport;

use anyhow::{anyhow, Result};
use eframe::egui;

fn main() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Teleop Controller")
            .with_inner_size([760.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Teleop Controller",
        options,
        Box::new(|cc| Ok(Box::new(app::TeleopApp::new(cc)))),
    )
    .map_err(|e| anyhow!("eframe error: {e}"))
}
