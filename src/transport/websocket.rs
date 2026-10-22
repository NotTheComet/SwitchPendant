use std::net::TcpStream;

use anyhow::{Context, Result};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

use super::{Config, Transport};
use crate::input::ControlState;

pub struct WsTransport {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl WsTransport {
    /// Plain `ws://` only (no TLS feature enabled).
    pub fn connect(cfg: &Config) -> Result<Self> {
        let (mut socket, _resp) = tungstenite::connect(cfg.ws_url.as_str())
            .with_context(|| format!("connecting to {}", cfg.ws_url))?;
        if let MaybeTlsStream::Plain(s) = socket.get_mut() {
            let _ = s.set_nodelay(true);
        }
        Ok(Self { socket })
    }
}

impl Transport for WsTransport {
    fn send(&mut self, state: &ControlState) -> Result<()> {
        self.socket
            .send(Message::text(state.to_json().to_string()))
            .context("websocket send")
    }
}

impl Drop for WsTransport {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}
