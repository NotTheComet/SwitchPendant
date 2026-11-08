//! The always-running part of the app. It owns the gamepad, the transport worker and the connection
//! state, so teleop keeps working while the window is closed. The window and the tray are just views.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::input::{ControlState, Input};
use crate::settings::{self, Settings};
use crate::transport::{Command, Config, Event, Worker};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected(String),
    /// The link dropped or failed; a retry is scheduled.
    Retrying(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Disconnected => "Disconnected".into(),
            Status::Connecting => "Connecting…".into(),
            Status::Connected(d) => format!("Connected: {d}"),
            Status::Retrying(e) => format!("Retrying: {e}"),
        }
    }
}

#[derive(Clone)]
pub struct Snapshot {
    pub status: Status,
    pub state: ControlState,
    pub sent: u64,
    pub last_error: Option<String>,
    pub cfg: Config,
    pub input_error: Option<String>,
}

pub enum CoreCmd {
    /// Connect with this config, or the current one if `None`.
    Connect(Option<Config>),
    Disconnect,
    SetConfig(Config),
    /// Keyboard fallback state from the window (used only when no gamepad is present).
    Keyboard(ControlState),
    /// Stop cleanly (sends a neutral command first) and acknowledge.
    Shutdown(Sender<()>),
}

#[derive(Clone)]
pub struct CoreHandle {
    tx: Sender<CoreCmd>,
    snap: Arc<Mutex<Snapshot>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl CoreHandle {
    pub fn send(&self, cmd: CoreCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.snap).clone()
    }

    pub fn shutdown(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self.tx.send(CoreCmd::Shutdown(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

/// `tx`/`rx` are the two ends of one channel, created by the caller so the tray can hold a sender.
/// `on_status` is called (on the core thread) whenever the status changes.
pub fn spawn(
    settings: Settings,
    tx: Sender<CoreCmd>,
    rx: Receiver<CoreCmd>,
    on_status: impl Fn(&Status) + Send + 'static,
) -> CoreHandle {
    let snap = Arc::new(Mutex::new(Snapshot {
        status: Status::Disconnected,
        state: ControlState::default(),
        sent: 0,
        last_error: None,
        cfg: settings.cfg.clone(),
        input_error: None,
    }));
    let snap_thread = snap.clone();
    thread::Builder::new()
        .name("teleop-core".into())
        .spawn(move || run(settings, rx, snap_thread, on_status))
        .expect("spawn core thread");
    CoreHandle { tx, snap }
}

const TICK: Duration = Duration::from_millis(5);
const KEYBOARD_TTL: Duration = Duration::from_millis(250);

fn run(
    settings: Settings,
    rx: Receiver<CoreCmd>,
    snap: Arc<Mutex<Snapshot>>,
    on_status: impl Fn(&Status),
) {
    let mut input = Input::new();
    let worker = Worker::spawn();

    let mut cfg = settings.cfg;
    let mut desired = settings.auto_connect;
    let mut status = Status::Disconnected;
    let mut last_error: Option<String> = None;
    let mut keyboard: Option<(ControlState, Instant)> = None;
    let mut sent: u64 = 0;
    let mut last_send = Instant::now();
    let mut retry_at: Option<Instant> = None;
    let mut retry_delay = Duration::from_secs(2);
    let mut dirty_since: Option<Instant> = None;
    let mut pushed_status: Option<Status> = None;

    let save = |cfg: &Config, desired: bool| {
        if let Err(e) = settings::save(&Settings { cfg: cfg.clone(), auto_connect: desired }) {
            eprintln!("teleop: could not save settings: {e:#}");
        }
    };

    if desired {
        worker.send(Command::Connect(cfg.clone()));
        status = Status::Connecting;
    }
    lock(&snap).input_error = input.init_error.clone();

    'main: loop {
        // ---- commands from the window / tray ----
        loop {
            match rx.try_recv() {
                Ok(CoreCmd::Connect(new_cfg)) => {
                    if let Some(c) = new_cfg {
                        cfg = c;
                    }
                    desired = true;
                    save(&cfg, desired);
                    retry_delay = Duration::from_secs(2);
                    retry_at = None;
                    last_error = None;
                    status = Status::Connecting;
                    worker.send(Command::Connect(cfg.clone()));
                }
                Ok(CoreCmd::Disconnect) => {
                    desired = false;
                    save(&cfg, desired);
                    retry_at = None;
                    status = Status::Disconnected;
                    worker.send(Command::Disconnect);
                }
                Ok(CoreCmd::SetConfig(c)) => {
                    if c != cfg {
                        cfg = c;
                        dirty_since = Some(Instant::now()); // debounce: saved once edits settle
                    }
                }
                Ok(CoreCmd::Keyboard(s)) => keyboard = Some((s, Instant::now())),
                Ok(CoreCmd::Shutdown(ack)) => {
                    // Does not touch `desired`, so auto-connect still applies on the next launch.
                    worker.send(Command::Disconnect);
                    thread::sleep(Duration::from_millis(200)); // let the neutral command go out
                    if dirty_since.is_some() {
                        save(&cfg, desired);
                    }
                    let _ = ack.send(());
                    break 'main;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'main,
            }
        }

        // ---- events from the transport worker ----
        while let Ok(ev) = worker.events.try_recv() {
            match ev {
                Event::Connected(desc) => {
                    status = Status::Connected(desc);
                    retry_delay = Duration::from_secs(2);
                    retry_at = None;
                    last_error = None;
                }
                Event::Disconnected => {
                    if !desired {
                        status = Status::Disconnected;
                    }
                }
                Event::Error(e) => {
                    last_error = Some(e.clone());
                    if desired {
                        status = Status::Retrying(e);
                        retry_at = Some(Instant::now() + retry_delay);
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(15));
                    } else {
                        status = Status::Disconnected;
                    }
                }
            }
        }

        if let Some(t) = retry_at {
            if desired && Instant::now() >= t {
                retry_at = None;
                status = Status::Connecting;
                worker.send(Command::Connect(cfg.clone()));
            }
        }

        if dirty_since.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
            dirty_since = None;
            save(&cfg, desired);
        }

        // ---- input -> transport ----
        let state = input
            .poll_gamepad()
            .or_else(|| {
                keyboard
                    .as_ref()
                    .filter(|(_, t)| t.elapsed() < KEYBOARD_TTL)
                    .map(|(s, _)| s.clone())
            })
            .unwrap_or_else(|| ControlState { source: "no input device".into(), ..Default::default() });

        let period = Duration::from_secs_f32(1.0 / cfg.rate_hz.max(1.0));
        if matches!(status, Status::Connected(_)) && last_send.elapsed() >= period {
            worker.send(Command::Send(state.clone()));
            last_send = Instant::now();
            sent += 1;
        }

        // ---- publish ----
        if pushed_status.as_ref() != Some(&status) {
            on_status(&status);
            pushed_status = Some(status.clone());
        }
        {
            let mut s = lock(&snap);
            s.status = status.clone();
            s.state = state;
            s.sent = sent;
            s.last_error = last_error.clone();
            s.cfg = cfg.clone();
        }

        thread::sleep(TICK);
    }
}
