//! GNOME/KDE tray icon via StatusNotifierItem (ksni). GNOME needs the "AppIndicator and
//! KStatusNotifierItem Support" shell extension to show it.
//!
//! Left click -> open the window. Right click -> menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Icon, MenuItem, ToolTip};

use crate::core::{CoreCmd, Status};
use crate::settings;

pub enum UiCmd {
    Show,
    Quit,
}

struct TeleopTray {
    status: Status,
    ui_tx: Sender<UiCmd>,
    core_tx: Sender<CoreCmd>,
}

impl ksni::Tray for TeleopTray {
    // Default MENU_ON_ACTIVATE = false: left click calls `activate`, right click shows the menu.

    fn id(&self) -> String {
        "teleop-controller".into()
    }

    fn title(&self) -> String {
        "Teleop Controller".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let color = match self.status {
            Status::Disconnected => (140, 140, 140),
            Status::Connecting => (240, 180, 0),
            Status::Connected(_) => (60, 200, 100),
            Status::Retrying(_) => (225, 65, 65),
        };
        [32, 64].into_iter().map(|size| render_icon(size, color)).collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Teleop Controller".into(),
            description: self.status.label(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.ui_tx.send(UiCmd::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let link_up = !matches!(self.status, Status::Disconnected);
        vec![
            StandardItem {
                label: "Open Teleop".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.ui_tx.send(UiCmd::Show);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem { label: self.status.label(), enabled: false, ..Default::default() }.into(),
            StandardItem {
                label: if link_up { "Disconnect" } else { "Connect" }.into(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.core_tx.send(if link_up { CoreCmd::Disconnect } else { CoreCmd::Connect(None) });
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Start at login".into(),
                checked: settings::autostart_enabled(),
                activate: Box::new(|_: &mut Self| {
                    if let Err(e) = settings::set_autostart(!settings::autostart_enabled()) {
                        eprintln!("teleop: autostart: {e:#}");
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.ui_tx.send(UiCmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Status disc with a white stick-and-crosshair mark, as ARGB32 (network byte order).
fn render_icon(size: i32, (r, g, b): (u8, u8, u8)) -> Icon {
    let s = size as f32;
    let c = (s - 1.0) / 2.0;
    let radius = s * 0.46;
    let arm = s * 0.065;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = dx.hypot(dy);
            let alpha = (radius - d + 0.5).clamp(0.0, 1.0);
            let in_arm = |a: f32, b: f32| a.abs() <= arm && (s * 0.20..=s * 0.38).contains(&b.abs());
            let white = d <= s * 0.12 || in_arm(dx, dy) || in_arm(dy, dx);
            let (pr, pg, pb) = if white { (255, 255, 255) } else { (r, g, b) };
            data.extend_from_slice(&[(alpha * 255.0) as u8, pr, pg, pb]);
        }
    }
    Icon { width: size, height: size, data }
}

/// Owns the tray. At login the shell's tray host may not be up yet, so creation is retried in the
/// background until it works.
#[derive(Clone)]
pub struct TrayHub {
    slot: Arc<Mutex<Option<Handle<TeleopTray>>>>,
    last_status: Arc<Mutex<Status>>,
    active: Arc<AtomicBool>,
}

impl TrayHub {
    pub fn start(ui_tx: Sender<UiCmd>, core_tx: Sender<CoreCmd>) -> Self {
        let hub = Self {
            slot: Arc::new(Mutex::new(None)),
            last_status: Arc::new(Mutex::new(Status::Disconnected)),
            active: Arc::new(AtomicBool::new(false)),
        };
        let h = hub.clone();
        thread::spawn(move || {
            let mut warned = false;
            loop {
                let status = h.last_status.lock().unwrap().clone();
                let tray = TeleopTray { status, ui_tx: ui_tx.clone(), core_tx: core_tx.clone() };
                match tray.spawn() {
                    Ok(handle) => {
                        *h.slot.lock().unwrap() = Some(handle);
                        h.active.store(true, Ordering::SeqCst);
                        // Catch up if the status changed while we were starting.
                        let st = h.last_status.lock().unwrap().clone();
                        h.push_status(&st);
                        return;
                    }
                    Err(e) => {
                        if !warned {
                            eprintln!(
                                "teleop: no system tray available yet ({e}); on GNOME install/enable the \
                                 AppIndicator extension. Retrying..."
                            );
                            warned = true;
                        }
                        thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        });
        hub
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn push_status(&self, status: &Status) {
        *self.last_status.lock().unwrap() = status.clone();
        if let Some(handle) = self.slot.lock().unwrap().as_ref() {
            let st = status.clone();
            let _ = handle.update(move |t| t.status = st);
        }
    }
}
