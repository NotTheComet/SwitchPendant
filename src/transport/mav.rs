use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use mavlink::{
    dialects::common::{MavAutopilot, MavMessage, MavType, HEARTBEAT_DATA, MANUAL_CONTROL_DATA},
    MavConnection, MavHeader,
};

use super::{Config, Transport};
use crate::input::ControlState;

pub struct MavTransport {
    conn: Box<dyn MavConnection<MavMessage> + Sync + Send>,
    header: MavHeader,
    target: u8,
    last_heartbeat: Option<Instant>,
}

impl MavTransport {
    pub fn connect(cfg: &Config) -> Result<Self> {
        let conn = mavlink::connect::<MavMessage>(&cfg.mav_addr)
            .map_err(|e| anyhow!("mavlink connect {}: {e}", cfg.mav_addr))?;
        Ok(Self {
            conn: Box::new(conn),
            // GCS-style identity: system 255, component 190 (mission planner).
            header: MavHeader { system_id: 255, component_id: 190, sequence: 0 },
            target: cfg.mav_target,
            last_heartbeat: None,
        })
    }
}

fn scale(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 1000.0).round() as i16
}

impl Transport for MavTransport {
    fn send(&mut self, s: &ControlState) -> Result<()> {
        // 1 Hz GCS heartbeat so the autopilot's GCS failsafe stays happy.
        if self.last_heartbeat.map_or(true, |t| t.elapsed() >= Duration::from_secs(1)) {
            self.last_heartbeat = Some(Instant::now());
            let hb = MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                mavtype: MavType::MAV_TYPE_GCS,
                autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
                ..Default::default()
            });
            self.header.sequence = self.header.sequence.wrapping_add(1);
            self.conn.send(&self.header, &hb).map_err(|e| anyhow!("mavlink send: {e:?}"))?;
        }

        let a = &s.axes;
        let buttons = s
            .buttons
            .iter()
            .take(16)
            .enumerate()
            .fold(0u16, |mask, (i, &pressed)| mask | ((pressed as u16) << i));

        // MANUAL_CONTROL: x/y/r are -1000..1000, z (thrust) is 0..1000.
        let msg = MavMessage::MANUAL_CONTROL(MANUAL_CONTROL_DATA {
            x: scale(a.ly),
            y: scale(a.lx),
            z: (500.0 + a.ry.clamp(-1.0, 1.0) * 500.0).round() as i16,
            r: scale(a.rx),
            buttons,
            target: self.target,
            ..Default::default()
        });

        self.header.sequence = self.header.sequence.wrapping_add(1);
        self.conn
            .send(&self.header, &msg)
            .map_err(|e| anyhow!("mavlink send: {e:?}"))?;
        Ok(())
    }
}
