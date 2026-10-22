//! Input handling: reads a gamepad via `gilrs`, falling back to the keyboard.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Key};
use gilrs::{Axis, Button, GamepadId, Gilrs};
use serde::Serialize;
use serde_json::{json, Map, Value};

pub const NUM_BUTTONS: usize = 15;

/// Order matters: it defines the MAVLink bitmask bit and the ROS `Joy.buttons` index.
pub const BUTTONS: [(Button, &str); NUM_BUTTONS] = [
    (Button::South, "south"),
    (Button::East, "east"),
    (Button::North, "north"),
    (Button::West, "west"),
    (Button::LeftTrigger, "lb"),
    (Button::RightTrigger, "rb"),
    (Button::Select, "select"),
    (Button::Start, "start"),
    (Button::Mode, "mode"),
    (Button::LeftThumb, "l3"),
    (Button::RightThumb, "r3"),
    (Button::DPadUp, "dpad_up"),
    (Button::DPadDown, "dpad_down"),
    (Button::DPadLeft, "dpad_left"),
    (Button::DPadRight, "dpad_right"),
];

/// Sticks are -1..1 (up/right positive), triggers are 0..1.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Axes {
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ControlState {
    pub axes: Axes,
    pub buttons: [bool; NUM_BUTTONS],
    pub source: String,
}

impl ControlState {
    pub fn to_json(&self) -> Value {
        let buttons: Map<String, Value> = BUTTONS
            .iter()
            .zip(self.buttons)
            .map(|((_, name), pressed)| (name.to_string(), Value::Bool(pressed)))
            .collect();
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        json!({
            "type": "teleop",
            "timestamp_ms": timestamp_ms,
            "axes": self.axes,
            "buttons": buttons,
        })
    }
}

pub struct Input {
    gilrs: Option<Gilrs>,
    active: Option<GamepadId>,
    pub init_error: Option<String>,
}

impl Input {
    pub fn new() -> Self {
        match Gilrs::new() {
            Ok(g) => Self { gilrs: Some(g), active: None, init_error: None },
            Err(e) => Self {
                gilrs: None,
                active: None,
                init_error: Some(format!("gamepad support unavailable: {e}")),
            },
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> ControlState {
        if let Some(g) = self.gilrs.as_mut() {
            // Draining events keeps gilrs' cached state current and tells us
            // which pad the user is actually touching.
            while let Some(ev) = g.next_event() {
                self.active = Some(ev.id);
            }
            let pad = self
                .active
                .and_then(|id| g.connected_gamepad(id))
                .or_else(|| g.gamepads().next().map(|(_, p)| p));
            if let Some(pad) = pad {
                return from_pad(&pad);
            }
        }
        from_keyboard(ctx)
    }
}

fn deadzone(v: f32) -> f32 {
    if v.abs() < 0.08 { 0.0 } else { v }
}

fn from_pad(pad: &gilrs::Gamepad) -> ControlState {
    let ax = |a| deadzone(pad.value(a));
    let trigger = |b| pad.button_data(b).map(|d| d.value()).unwrap_or(0.0);

    let mut buttons = [false; NUM_BUTTONS];
    for (i, (b, _)) in BUTTONS.iter().enumerate() {
        buttons[i] = pad.is_pressed(*b);
    }

    ControlState {
        axes: Axes {
            lx: ax(Axis::LeftStickX),
            ly: ax(Axis::LeftStickY),
            rx: ax(Axis::RightStickX),
            ry: ax(Axis::RightStickY),
            lt: trigger(Button::LeftTrigger2),
            rt: trigger(Button::RightTrigger2),
        },
        buttons,
        source: pad.name().to_string(),
    }
}

/// WASD = left stick, arrows = right stick, Space = south, Enter = start.
fn from_keyboard(ctx: &egui::Context) -> ControlState {
    let source = "keyboard (WASD / arrows / Space / Enter)".to_string();
    if ctx.egui_wants_keyboard_input() {
        return ControlState { source, ..Default::default() };
    }
    ctx.input(|i| {
        let k = |key| i.key_down(key);
        let axis = |pos, neg| (k(pos) as i8 - k(neg) as i8) as f32;

        let mut buttons = [false; NUM_BUTTONS];
        buttons[0] = k(Key::Space); // south
        buttons[7] = k(Key::Enter); // start

        ControlState {
            axes: Axes {
                lx: axis(Key::D, Key::A),
                ly: axis(Key::W, Key::S),
                rx: axis(Key::ArrowRight, Key::ArrowLeft),
                ry: axis(Key::ArrowUp, Key::ArrowDown),
                lt: 0.0,
                rt: 0.0,
            },
            buttons,
            source,
        }
    })
}
