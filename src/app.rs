use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Color32, RichText, Sense, Stroke, Vec2};

use crate::input::{ControlState, Input, BUTTONS};
use crate::transport::{Command, Config, Event, Protocol, Worker};

enum Status {
    Disconnected,
    Connecting,
    Connected(String),
}

pub struct TeleopApp {
    cfg: Config,
    input: Input,
    state: ControlState,
    worker: Worker,
    status: Status,
    last_error: Option<String>,
    last_send: Instant,
    sent: u64,
}

impl TeleopApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            cfg: Config::default(),
            input: Input::new(),
            state: ControlState::default(),
            worker: Worker::spawn(cc.egui_ctx.clone()),
            status: Status::Disconnected,
            last_error: None,
            last_send: Instant::now(),
            sent: 0,
        }
    }

    fn connection_panel(&mut self, ui: &mut egui::Ui) {
        let idle = matches!(self.status, Status::Disconnected);

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
                    self.worker.send(Command::Connect(self.cfg.clone()));
                    self.status = Status::Connecting;
                    self.last_error = None;
                }
            } else if ui.button("Disconnect").clicked() {
                self.worker.send(Command::Disconnect);
                self.status = Status::Disconnected;
            }
        });

        ui.horizontal(|ui| match &self.status {
            Status::Disconnected => {
                ui.colored_label(Color32::GRAY, "● Disconnected");
            }
            Status::Connecting => {
                ui.colored_label(Color32::YELLOW, "● Connecting…");
            }
            Status::Connected(d) => {
                ui.colored_label(Color32::from_rgb(60, 200, 100), format!("● Connected: {d}"));
                ui.label(format!("({} msgs sent)", self.sent));
            }
        });
        if let Some(err) = &self.last_error {
            ui.colored_label(Color32::from_rgb(230, 80, 80), err);
        }
    }
}

impl eframe::App for TeleopApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        while let Ok(ev) = self.worker.events.try_recv() {
            match ev {
                Event::Connected(desc) => self.status = Status::Connected(desc),
                Event::Disconnected => self.status = Status::Disconnected,
                Event::Error(e) => {
                    self.status = Status::Disconnected;
                    self.last_error = Some(e);
                }
            }
        }

        self.state = self.input.poll(ui);

        let period = Duration::from_secs_f32(1.0 / self.cfg.rate_hz.max(1.0));
        if matches!(self.status, Status::Connected(_)) && self.last_send.elapsed() >= period {
            self.worker.send(Command::Send(self.state.clone()));
            self.last_send = Instant::now();
            self.sent += 1;
        }

        egui::Panel::top("connection").show(ui, |ui| {
            ui.add_space(4.0);
            self.connection_panel(ui);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(e) = &self.input.init_error {
                ui.colored_label(Color32::from_rgb(230, 80, 80), e);
            }
            ui.label(format!("Input: {}", self.state.source));
            ui.separator();

            let a = &self.state.axes;
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
                for ((_, name), &pressed) in BUTTONS.iter().zip(&self.state.buttons) {
                    let (bg, fg) = if pressed {
                        (Color32::from_rgb(40, 160, 80), Color32::WHITE)
                    } else {
                        (ui.visuals().faint_bg_color, ui.visuals().weak_text_color())
                    };
                    ui.label(RichText::new(format!(" {name} ")).monospace().color(fg).background_color(bg));
                }
            });
        });

        // Gamepads are polled, not event-driven, so keep repainting.
        ui.request_repaint_after(Duration::from_millis(10));
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
