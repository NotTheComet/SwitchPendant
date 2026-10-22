//! Transports run on a worker thread so slow connects / sends never block the GUI.

mod mav;
mod ros2;
mod websocket;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use eframe::egui;

use crate::input::ControlState;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protocol {
    WebSocket,
    Mavlink,
    Ros2,
}

impl Protocol {
    pub const ALL: [Protocol; 3] = [Protocol::WebSocket, Protocol::Mavlink, Protocol::Ros2];

    pub fn label(self) -> &'static str {
        match self {
            Protocol::WebSocket => "WebSocket (JSON)",
            Protocol::Mavlink => "MAVLink",
            Protocol::Ros2 => "ROS 2 (rosbridge)",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub protocol: Protocol,
    pub ws_url: String,
    /// e.g. `udpout:127.0.0.1:14550`, `udpin:0.0.0.0:14550`, `tcpout:host:5760`, `serial:/dev/ttyUSB0:57600`
    pub mav_addr: String,
    pub mav_target: u8,
    /// rosbridge_server websocket (ROS 1 or ROS 2)
    pub ros_url: String,
    pub ros_cmd_topic: String,
    pub ros_joy_topic: String,
    pub max_lin: f64,
    pub max_ang: f64,
    pub rate_hz: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            protocol: Protocol::WebSocket,
            ws_url: "ws://127.0.0.1:9001".into(),
            mav_addr: "udpout:127.0.0.1:14550".into(),
            mav_target: 1,
            ros_url: "ws://127.0.0.1:9090".into(),
            ros_cmd_topic: "/cmd_vel".into(),
            ros_joy_topic: "/joy".into(),
            max_lin: 1.0,
            max_ang: 1.0,
            rate_hz: 50.0,
        }
    }
}

impl Config {
    pub fn describe(&self) -> String {
        match self.protocol {
            Protocol::WebSocket => format!("WebSocket {}", self.ws_url),
            Protocol::Mavlink => format!("MAVLink {}", self.mav_addr),
            Protocol::Ros2 => format!("ROS via {} ({}, {})", self.ros_url, self.ros_cmd_topic, self.ros_joy_topic),
        }
    }
}

pub trait Transport {
    fn send(&mut self, state: &ControlState) -> Result<()>;
}

fn connect(cfg: &Config) -> Result<Box<dyn Transport>> {
    Ok(match cfg.protocol {
        Protocol::WebSocket => Box::new(websocket::WsTransport::connect(cfg)?),
        Protocol::Mavlink => Box::new(mav::MavTransport::connect(cfg)?),
        Protocol::Ros2 => Box::new(ros2::Ros2Transport::connect(cfg)?),
    })
}

pub enum Command {
    Connect(Config),
    Disconnect,
    Send(ControlState),
}

pub enum Event {
    Connected(String),
    Disconnected,
    Error(String),
}

pub struct Worker {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, events) = mpsc::channel();
        thread::spawn(move || run(cmd_rx, ev_tx, ctx));
        Self { tx, events }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

fn run(cmd_rx: Receiver<Command>, ev_tx: Sender<Event>, ctx: egui::Context) {
    // Created inside this thread, so transports don't need to be `Send`.
    let mut transport: Option<Box<dyn Transport>> = None;
    let emit = |e: Event| {
        let _ = ev_tx.send(e);
        ctx.request_repaint();
    };

    while let Ok(first) = cmd_rx.recv() {
        let mut batch = vec![first];
        batch.extend(cmd_rx.try_iter());
        // If we fell behind, only the newest control state is worth sending.
        let last_send = batch.iter().rposition(|c| matches!(c, Command::Send(_)));

        for (i, cmd) in batch.into_iter().enumerate() {
            match cmd {
                Command::Connect(cfg) => match connect(&cfg) {
                    Ok(t) => {
                        transport = Some(t);
                        emit(Event::Connected(cfg.describe()));
                    }
                    Err(e) => emit(Event::Error(format!("{e:#}"))),
                },
                Command::Disconnect => {
                    if let Some(mut t) = transport.take() {
                        // Leave the vehicle with a neutral command.
                        let _ = t.send(&ControlState::default());
                    }
                    emit(Event::Disconnected);
                }
                Command::Send(state) => {
                    if Some(i) != last_send {
                        continue;
                    }
                    if let Some(t) = transport.as_mut() {
                        if let Err(e) = t.send(&state) {
                            transport = None;
                            emit(Event::Error(format!("send failed: {e:#}")));
                        }
                    }
                }
            }
        }
    }
}
