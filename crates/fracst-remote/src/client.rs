//! A blocking client: connect to a `fracst-server` (directly, or through a
//! deployment such as fal.ai) and render frames on it.

use crate::codec;
use crate::protocol::{ClientMsg, FrameHeader, PROTOCOL, ServerMsg};
use fracst_core::{Frame, Progress, Settings, View};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// How long to wait for the server to accept and say hello. Serverless
/// deployments may first have to start a machine.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);
/// How often a waiting render checks for cancellation.
const POLL: Duration = Duration::from_millis(15);

pub struct Client {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// Binary messages still to skip: the blocks of a frame nobody waits for.
    skip: u32,
    /// The server's pixel engine, e.g. "CUDA: NVIDIA H100 80GB HBM3 (sm_90, 132 SMs)".
    pub engine: String,
    /// The server's fracst version.
    pub version: String,
    /// About the last frame received.
    pub last: Option<Transfer>,
}

/// What the last frame cost.
#[derive(Clone, Copy, Debug)]
pub struct Transfer {
    /// Compressed bytes received.
    pub bytes: u64,
    /// Seconds the server spent rendering and compressing.
    pub render_s: f64,
    pub encode_s: f64,
}

fn set_timeout(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, timeout: Duration) -> std::io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(timeout)),
        MaybeTlsStream::Rustls(s) => s.get_mut().set_read_timeout(Some(timeout)),
        _ => Ok(()),
    }
}

fn describe(e: tungstenite::Error) -> String {
    match e {
        tungstenite::Error::Http(response) => {
            let body = response.body().as_deref().map(String::from_utf8_lossy).unwrap_or_default();
            // A gateway also puts the reason in a header, in case the body was not read.
            let reason = match body.trim() {
                "" => response.headers().get("x-fracst-error").and_then(|v| v.to_str().ok()).unwrap_or("").to_string(),
                body => body.to_string(),
            };
            format!("server answered {}: {reason}", response.status())
        }
        e => e.to_string(),
    }
}

impl Client {
    /// Connect to `url` (`ws://` or `wss://`). `key` authenticates: sent as
    /// `Authorization: Key ...` to fal.ai, as `Modal-Key` and `Modal-Secret`
    /// to Modal (a proxy auth token given as `wk-…:ws-…`), and as
    /// `Authorization: Bearer ...` elsewhere (a fracst server or gateway).
    pub fn connect(url: &str, key: Option<&str>) -> Result<Client, String> {
        Self::connect_with_status(url, key, &mut |_| {})
    }

    /// [`Client::connect`], passing on status reports (from a gateway
    /// starting a GPU, say) while waiting for the server.
    pub fn connect_with_status(url: &str, key: Option<&str>, status: &mut dyn FnMut(&str)) -> Result<Client, String> {
        let mut request = url.into_client_request().map_err(|e| format!("bad URL {url}: {e}"))?;
        let uri = request.uri().clone();
        let host = uri.host().ok_or("the URL has no host")?.to_string();
        if let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) {
            let fal = host == "fal.run" || host.ends_with(".fal.run");
            let headers = request.headers_mut();
            let header = |v: &str| HeaderValue::from_str(v).map_err(|e| e.to_string());
            if host.ends_with(".modal.run") {
                let (id, secret) = key.split_once(':').ok_or("for Modal, give the proxy auth token as wk-…:ws-…")?;
                headers.insert("Modal-Key", header(id)?);
                headers.insert("Modal-Secret", header(secret)?);
            } else {
                let value = format!("{} {key}", if fal { "Key" } else { "Bearer" });
                headers.insert("Authorization", header(&value)?);
            }
        }
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve {host}: {e}"))?
            .next()
            .ok_or_else(|| format!("cannot resolve {host}"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
            .map_err(|e| format!("cannot connect to {host}:{port}: {e}"))?;
        stream.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let (mut socket, _) = tungstenite::client_tls(request, stream).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => describe(e),
            tungstenite::HandshakeError::Interrupted(_) => "handshake interrupted".into(),
        })?;
        let hello = loop {
            match socket.read().map_err(describe)? {
                Message::Text(text) => match serde_json::from_str::<ServerMsg>(&text) {
                    Ok(ServerMsg::Status { message }) => status(&message),
                    Ok(ServerMsg::Keepalive) => {}
                    _ => break text,
                },
                Message::Close(frame) => {
                    return Err(frame.map_or("the server closed the connection".into(), |f| f.reason.to_string()));
                }
                _ => {}
            }
        };
        let (engine, version) = match serde_json::from_str(&hello) {
            Ok(ServerMsg::Hello { protocol, version, engine }) if protocol == PROTOCOL => (engine, version),
            Ok(ServerMsg::Hello { protocol, version, .. }) => {
                return Err(format!("server fracst {version} speaks protocol {protocol}, this client {PROTOCOL}"));
            }
            Ok(ServerMsg::Error { message, .. }) => return Err(message),
            _ => return Err(format!("unexpected greeting: {hello}")),
        };
        set_timeout(&mut socket, POLL).map_err(|e| e.to_string())?;
        Ok(Client { socket, next_id: 1, skip: 0, engine, version, last: None })
    }

    fn send(&mut self, msg: &ClientMsg) -> Result<(), String> {
        let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        self.socket.send(Message::text(text)).map_err(describe)
    }

    fn set_nonblocking(&mut self, on: bool) -> std::io::Result<()> {
        match self.socket.get_mut() {
            MaybeTlsStream::Plain(s) => s.set_nonblocking(on),
            MaybeTlsStream::Rustls(s) => s.get_mut().set_nonblocking(on),
            _ => Ok(()),
        }
    }

    /// Handle whatever arrived since the last frame (leftovers of abandoned
    /// requests, or the server saying goodbye) without waiting.
    fn drain(&mut self) -> Result<(), String> {
        self.set_nonblocking(true).map_err(|e| e.to_string())?;
        let result = loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => match serde_json::from_str::<ServerMsg>(&text) {
                    Ok(ServerMsg::Frame(h)) => self.skip += h.blocks,
                    Ok(ServerMsg::Error { id: None, message }) => break Err(message),
                    _ => {}
                },
                Ok(Message::Binary(_)) => self.skip = self.skip.saturating_sub(1),
                Ok(Message::Close(frame)) => {
                    break Err(frame.map_or("the server closed the connection".into(), |f| f.reason.to_string()));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => break Ok(()),
                Err(e) => break Err(describe(e)),
            }
        };
        let _ = self.set_nonblocking(false);
        result
    }

    /// The next message, or `None` if none arrived within [`POLL`].
    fn read(&mut self) -> Result<Option<Message>, String> {
        match self.socket.read() {
            Ok(m) => Ok(Some(m)),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => Err(describe(e)),
        }
    }

    /// Render `view` on the server. `Ok(None)` if cancelled through
    /// `progress`, which also receives the server's progress reports.
    pub fn render(&mut self, view: &View, settings: &Settings, progress: &Progress) -> Result<Option<Frame>, String> {
        self.drain()?;
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Render { id, view: view.into(), settings: settings.into() })?;
        let mut header: Option<FrameHeader> = None;
        let mut blocks = Vec::new();
        loop {
            if progress.cancelled() {
                // Blocks of this frame that still arrive are skipped by the next render.
                if let Some(h) = &header {
                    self.skip += h.blocks - blocks.len() as u32;
                }
                self.send(&ClientMsg::Cancel { id })?;
                return Ok(None);
            }
            let msg = match self.read()? {
                Some(m) => m,
                None => continue,
            };
            match msg {
                Message::Text(text) => match serde_json::from_str::<ServerMsg>(&text) {
                    Ok(ServerMsg::Progress { id: p, stage, fraction }) if p == id && header.is_none() => {
                        progress.set_stage(stage, 1000);
                        progress.done.store((fraction.clamp(0.0, 1.0) * 1000.0) as u64, Ordering::Relaxed);
                    }
                    Ok(ServerMsg::Frame(h)) if h.id == id => header = Some(h),
                    // A frame for an earlier, abandoned request.
                    Ok(ServerMsg::Frame(h)) => self.skip += h.blocks,
                    Ok(ServerMsg::Error { id: e, message }) if e.is_none_or(|e| e == id) => return Err(message),
                    Ok(_) => {}
                    Err(e) => return Err(format!("bad message from server: {e}")),
                },
                Message::Binary(data) => {
                    if self.skip > 0 {
                        self.skip -= 1;
                    } else if header.is_some() {
                        blocks.push(data);
                    }
                }
                Message::Close(_) => return Err("the server closed the connection".into()),
                _ => {}
            }
            if let Some(h) = header.as_ref().filter(|h| blocks.len() == h.blocks as usize) {
                let mut frame = codec::decode(h.width, h.height, h.maxiter, h.de, &blocks)?;
                frame.stats = h.stats.to_stats();
                self.last = Some(Transfer { bytes: h.bytes, render_s: h.render_s, encode_s: h.encode_s });
                return Ok(Some(frame));
            }
        }
    }
}
