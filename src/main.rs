mod app;
mod core;
mod input;
mod settings;
mod transport;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use eframe::egui;

use crate::core::CoreHandle;
use crate::tray::{TrayHub, UiCmd};

pub type SharedUiRx = Arc<Mutex<Receiver<UiCmd>>>;

fn main() -> Result<()> {
    // `--background` (used by the autostart entry): start in the tray without opening the window.
    let background = std::env::args().any(|a| a == "--background");

    let settings = settings::load();
    let (core_tx, core_rx) = mpsc::channel();
    let (ui_tx, ui_rx) = mpsc::channel();

    let hub = TrayHub::start(ui_tx, core_tx.clone());
    let core = core::spawn(settings, core_tx, core_rx, {
        let hub = hub.clone();
        move |status| hub.push_status(status)
    });

    let ui_rx: SharedUiRx = Arc::new(Mutex::new(ui_rx));
    let quit = Arc::new(AtomicBool::new(false));

    let mut show_window = !background;
    if background {
        // At login the shell may not have loaded its tray host yet; give it a while. If it never
        // appears, open the window instead so the app isn't running invisibly.
        let deadline = Instant::now() + Duration::from_secs(30);
        while !hub.is_active() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(200));
        }
        show_window = !hub.is_active();
    }

    loop {
        if show_window {
            if let Err(e) = run_window(core.clone(), ui_rx.clone(), quit.clone()) {
                eprintln!("teleop: window error: {e:#}");
            }
            // Closing the window only hides the app, but only if there is a tray to bring it back.
            if quit.load(Ordering::SeqCst) || !hub.is_active() {
                break;
            }
        }

        // Window closed (or never opened): sleep until the tray asks for something.
        let cmd = ui_rx.lock().unwrap().recv();
        match cmd {
            Ok(UiCmd::Show) => show_window = true,
            Ok(UiCmd::Quit) | Err(_) => break,
        }
    }

    core.shutdown();
    Ok(())
}

/// Runs one window session. eframe returns when the window is closed, so the window can be
/// recreated later without restarting the process.
fn run_window(core: CoreHandle, ui_rx: SharedUiRx, quit: Arc<AtomicBool>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Teleop Controller")
            .with_app_id("teleop-controller")
            .with_inner_size([700.0, 430.0]),
        run_and_return: true,
        ..Default::default()
    };

    eframe::run_native(
        "Teleop Controller",
        options,
        Box::new(move |_cc| Ok(Box::new(app::TeleopApp::new(core, ui_rx, quit)))),
    )
    .map_err(|e| anyhow!("eframe error: {e}"))
}
