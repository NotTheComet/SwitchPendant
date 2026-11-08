//! The window: a view onto the always-running core. Closing it does not stop teleop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, vec2, Color32, RichText, Sense, Stroke, Vec2};

use crate::core::{CoreCmd, CoreHandle, Snapshot, Status};
use crate::input::{self, BUTTONS};
use crate::settings;
use crate::transport::{Config, Protocol};
use crate::tray::UiCmd;
use crate::SharedUiRx;

pub struct TeleopApp {
    core: CoreHandle,
    ui_rx: SharedUiRx,
    quit: Arc<AtomicBool>,

    snap: Snapshot,
    /// Editable copy of the config; pushed to the core when it changes.
    cfg: Config,
    synced_cfg: Config,
    keyboard_was_active: bool,
    autostart: bool,
    ui_error: Option<String>,
}

impl TeleopApp {
    pub fn new(core: CoreHandle, ui_rx: SharedUiRx, quit: Arc<AtomicBool>) -> Self {
        let snap = core.snapshot();
        Self {
            cfg: snap.cfg.clone(),
            synced_cfg: snap.cfg.clone(),
            snap,
            core,
            ui_rx,
            quit,
            keyboard_was_active: false,
            autostart: settings::autostart_enabled(),
            ui_error: None,
        }
    }

    fn sync_config(&mut self) {
        if self.cfg != self.synced_cfg {
            self.core.send(CoreCmd::SetConfig(self.cfg.clone()));
            self.synced_cfg = self.cfg.clone();
        }
    }

    fn connection_panel(&mut self, ui: &mut egui::Ui) {
        let idle = matches!(self.snap.status, Status::Disconnected);

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("protocol")
                    .selected_text(self.cfg.protocol.label())
                    .show_ui(ui, |ui| {
                        for p in Protocol::ALL {
                            ui.selectable_value(&mut self.cfg.protocol, p, p.label());
                        }
                    });

                match self.cfg.protocol {
                    Protocol::WebSocket => {
                        ui.label("URL");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.ws_url).desired_width(240.0));
                    }
                    Protocol::Mavlink => {
                        ui.label("Address");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.mav_addr).desired_width(200.0));
                        ui.label("Target sys");
                        ui.add(egui::DragValue::new(&mut self.cfg.mav_target).range(0..=255));
                    }
                    Protocol::Ros2 => {
                        ui.label("rosbridge");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.ros_url).desired_width(170.0));
                        ui.label("Twist");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.ros_cmd_topic).desired_width(100.0));
                        ui.label("Joy");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.ros_joy_topic).desired_width(80.0));
                        ui.label("Max lin");
                        ui.add(egui::DragValue::new(&mut self.cfg.max_lin).speed(0.05).range(0.0..=20.0));
                        ui.label("Max ang");
                        ui.add(egui::DragValue::new(&mut self.cfg.max_ang).speed(0.05).range(0.0..=20.0));
                    }
                }
            });

            ui.label("Rate");
            ui.add(egui::DragValue::new(&mut self.cfg.rate_hz).range(1.0..=200.0).suffix(" Hz"));

            if idle {
                if ui.button("Connect").clicked() {
                    self.core.send(CoreCmd::Connect(Some(self.cfg.clone())));
                    self.synced_cfg = self.cfg.clone();
                }
            } else if ui.button("Disconnect").clicked() {
                self.core.send(CoreCmd::Disconnect);
            }
        });

        ui.horizontal(|ui| match &self.snap.status {
            Status::Disconnected => {
                ui.colored_label(Color32::GRAY, "● Disconnected");
            }
            Status::Connecting => {
                ui.colored_label(Color32::YELLOW, "● Connecting…");
            }
            Status::Connected(d) => {
                ui.colored_label(Color32::from_rgb(60, 200, 100), format!("● Connected: {d}"));
                ui.label(format!("({} msgs sent)", self.snap.sent));
            }
            Status::Retrying(_) => {
                ui.colored_label(Color32::from_rgb(230, 80, 80), "● Link down, retrying…");
            }
        });
        if let Some(err) = self.snap.last_error.as_ref().or(self.ui_error.as_ref()) {
            ui.colored_label(Color32::from_rgb(230, 80, 80), err);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.autostart, "Start at login").changed() {
                self.ui_error = settings::set_autostart(self.autostart)
                    .err()
                    .map(|e| format!("autostart: {e:#}"));
                self.autostart = settings::autostart_enabled();
            }
            ui.weak("Closing this window keeps Teleop running in the tray.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Quit").clicked() {
                    self.quit.store(true, Ordering::SeqCst);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

impl eframe::App for TeleopApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Requests from the tray while the window is already open.
        if let Ok(rx) = self.ui_rx.try_lock() {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    UiCmd::Show => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus),
                    UiCmd::Quit => {
                        self.quit.store(true, Ordering::SeqCst);
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // Keyboard fallback goes to the core, which uses it only when no gamepad is present.
        let kb = input::from_keyboard(ui.ctx());
        let kb_active = !kb.is_idle();
        if kb_active || self.keyboard_was_active {
            self.core.send(CoreCmd::Keyboard(kb));
        }
        self.keyboard_was_active = kb_active;

        self.snap = self.core.snapshot();

        egui::Panel::top("connection").show_inside(ui, |ui| {
            ui.add_space(4.0);
            self.connection_panel(ui);
            ui.add_space(4.0);
        });

        egui::Panel::bottom("footer").show_inside(ui, |ui| {
            ui.add_space(4.0);
            self.footer(ui);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(e) = &self.snap.input_error {
                ui.colored_label(Color32::from_rgb(230, 80, 80), e);
            }
            ui.label(format!("Input: {}", self.snap.state.source));
            ui.separator();

            let a = &self.snap.state.axes;
            ui.horizontal(|ui| {
                stick(ui, "Left stick", a.lx, a.ly);
                ui.add_space(24.0);
                stick(ui, "Right stick", a.rx, a.ry);
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Triggers").strong());
                    for (name, v) in [("LT", a.lt), ("RT", a.rt)] {
                        ui.add(
                            egui::ProgressBar::new(v.clamp(0.0, 1.0))
                                .desired_width(160.0)
                                .text(format!("{name} {v:.2}")),
                        );
                    }
                });
            });

            ui.separator();
            ui.label(RichText::new("Buttons").strong());
            ui.horizontal_wrapped(|ui| {
                for ((_, name), &pressed) in BUTTONS.iter().zip(&self.snap.state.buttons) {
                    let (bg, fg) = if pressed {
                        (Color32::from_rgb(40, 160, 80), Color32::WHITE)
                    } else {
                        (ui.visuals().faint_bg_color, ui.visuals().weak_text_color())
                    };
                    ui.label(RichText::new(format!(" {name} ")).monospace().color(fg).background_color(bg));
                }
            });
        });

        self.sync_config();

        // Core state changes without egui events, so keep refreshing while the window is open.
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn stick(ui: &mut egui::Ui, label: &str, x: f32, y: f32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).strong());

        let size = 150.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let painter = ui.painter_at(rect);
        let c = rect.center();
        let r = size / 2.0 - 8.0;

        let line = Stroke::new(1.0, ui.visuals().weak_text_color());
        painter.circle_stroke(c, r, line);
        painter.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], line);
        painter.line_segment([c - vec2(0.0, r), c + vec2(0.0, r)], line);

        let v = vec2(x, -y); // screen Y points down
        let v = if v.length() > 1.0 { v.normalized() } else { v };
        painter.circle_filled(c + v * r, 8.0, Color32::from_rgb(80, 180, 255));

        ui.monospace(format!("x {x:+.2}\ny {y:+.2}"));
    });
}
