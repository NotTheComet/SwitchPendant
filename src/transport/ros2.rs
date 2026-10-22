//! ROS 2 via roslibrust's rosbridge backend (needs `rosbridge_server` running, no ROS install on this machine).
//!
//! roslibrust is async, so this transport owns a tokio runtime and a publisher task;
//! `send()` just queues the latest state to that task.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use roslibrust::rosbridge::ClientHandle;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use super::{Config, Transport};
use crate::input::ControlState;

// Rust types are generated at compile time from the vendored .msg files in ./msgs
mod msgs {
    roslibrust_codegen_macro::generate_ros_types!("msgs");
}
use msgs::{
    geometry_msgs::{Twist, Vector3},
    sensor_msgs::Joy,
};

pub struct Ros2Transport {
    tx: UnboundedSender<ControlState>,
    err_rx: mpsc::Receiver<String>,
    // Declared last so it is dropped last (shuts the publisher task down).
    _rt: Runtime,
}

impl Ros2Transport {
    pub fn connect(cfg: &Config) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("creating tokio runtime")?;

        let (cmd_topic, joy_topic) = (cfg.ros_cmd_topic.clone(), cfg.ros_joy_topic.clone());
        let url = cfg.ros_url.clone();

        let (client, twist_pub, joy_pub) = rt.block_on(async {
            let client = tokio::time::timeout(Duration::from_secs(5), ClientHandle::new(url.as_str()))
                .await
                .map_err(|_| anyhow!("timed out connecting to rosbridge at {url}"))?
                .map_err(|e| anyhow!("rosbridge connect {url}: {e}"))?;
            let twist_pub = client
                .advertise::<Twist>(&cmd_topic)
                .await
                .map_err(|e| anyhow!("advertise {cmd_topic}: {e}"))?;
            let joy_pub = client
                .advertise::<Joy>(&joy_topic)
                .await
                .map_err(|e| anyhow!("advertise {joy_topic}: {e}"))?;
            Ok::<_, anyhow::Error>((client, twist_pub, joy_pub))
        })?;

        let (tx, mut rx) = unbounded_channel::<ControlState>();
        let (err_tx, err_rx) = mpsc::channel::<String>();
        let (max_lin, max_ang) = (cfg.max_lin, cfg.max_ang);

        rt.spawn(async move {
            let _client = client; // keep the websocket alive for the publishers
            while let Some(s) = rx.recv().await {
                let (twist, joy) = to_msgs(&s, max_lin, max_ang);
                let res: Result<()> = async {
                    twist_pub.publish(&twist).await.map_err(|e| anyhow!("publish twist: {e}"))?;
                    joy_pub.publish(&joy).await.map_err(|e| anyhow!("publish joy: {e}"))?;
                    Ok(())
                }
                .await;
                if let Err(e) = res {
                    let _ = err_tx.send(format!("{e:#}"));
                    break;
                }
            }
        });

        Ok(Self { tx, err_rx, _rt: rt })
    }
}

/// Left stick Y = forward speed, right stick X = yaw (right = clockwise = negative z).
fn to_msgs(s: &ControlState, max_lin: f64, max_ang: f64) -> (Twist, Joy) {
    let a = &s.axes;
    let twist = Twist {
        linear: Vector3 { x: a.ly as f64 * max_lin, y: 0.0, z: 0.0 },
        angular: Vector3 { x: 0.0, y: 0.0, z: -(a.rx as f64) * max_ang },
    };
    let joy = Joy {
        header: Default::default(),
        axes: vec![a.lx, a.ly, a.rx, a.ry, a.lt, a.rt],
        buttons: s.buttons.iter().map(|&b| b as i32).collect(),
    };
    (twist, joy)
}

impl Transport for Ros2Transport {
    fn send(&mut self, state: &ControlState) -> Result<()> {
        if let Ok(e) = self.err_rx.try_recv() {
            bail!("{e}");
        }
        self.tx
            .send(state.clone())
            .map_err(|_| anyhow!("rosbridge publisher task stopped"))
    }
}
