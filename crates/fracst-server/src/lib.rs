//! fracst-server: renders frames for remote fracst clients over a
//! WebSocket, iterating pixels on a CUDA GPU when one is available.
//!
//! Each connection gets its own renderer (so its own reference orbit and
//! BLA cache); connections share the GPU. A newer request on a connection
//! cancels the ones in flight, unless it is queued behind them.

use fracst_core::{PixelEngine, Progress, Renderer, Settings, View};
use fracst_remote::codec;
use fracst_remote::protocol::{ClientMsg, FEATURE_QUEUE, FrameHeader, PROTOCOL, ServerMsg, StatsMsg};
use std::collections::VecDeque;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

/// How often an otherwise silent connection gets a keepalive message.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// What every connection shares.
pub struct Config {
    /// Pixel engine for all connections; `None` renders on the CPU.
    pub engine: Option<Arc<dyn PixelEngine>>,
    /// Shown to clients, e.g. "CUDA: NVIDIA H100 80GB HBM3 (sm_90, 132 SMs)".
    pub engine_name: String,
    /// Clients must send `Authorization: Bearer TOKEN` when set.
    pub token: Option<String>,
    /// Largest image a client may request, in pixels.
    pub max_pixels: u64,
    /// Close connections with no requests for this long (they may keep a
    /// paid GPU running).
    pub idle_timeout: Option<Duration>,
}

/// Accept connections on `listener` forever, one thread each.
pub fn run(listener: TcpListener, config: Config) {
    let config = Arc::new(config);
    for stream in listener.incoming().flatten() {
        let config = config.clone();
        std::thread::spawn(move || {
            let peer = stream.peer_addr().map_or("?".into(), |a| a.to_string());
            match serve(stream, &config) {
                Ok(()) => eprintln!("{peer}: disconnected"),
                Err(e) => eprintln!("{peer}: {e}"),
            }
        });
    }
}

struct Job {
    id: u64,
    view: View,
    settings: Settings,
    progress: Arc<Progress>,
}

enum Done {
    Frame(Box<FrameHeader>, Vec<Vec<u8>>),
    Error(u64, String),
}

fn send(socket: &mut WebSocket<TcpStream>, msg: &ServerMsg) -> Result<(), String> {
    let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    socket.send(Message::text(text)).map_err(|e| e.to_string())
}

/// One client connection: requests come in on the socket, a worker thread renders.
// The handshake callback's error type is tungstenite's.
#[allow(clippy::result_large_err)]
fn serve(stream: TcpStream, shared: &Config) -> Result<(), String> {
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let check_token = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let Some(token) = &shared.token else { return Ok(resp) };
        let given = req.headers().get("authorization").and_then(|v| v.to_str().ok());
        if given == Some(format!("Bearer {token}").as_str()) {
            Ok(resp)
        } else {
            let mut denied = ErrorResponse::new(Some("missing or wrong token".into()));
            *denied.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED;
            Err(denied)
        }
    };
    let mut socket = tungstenite::accept_hdr(stream, check_token).map_err(|e| format!("handshake: {e}"))?;
    let hello = ServerMsg::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
        engine: shared.engine_name.clone(),
        features: vec![FEATURE_QUEUE.into()],
    };
    send(&mut socket, &hello)?;
    socket.get_mut().set_read_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;

    let (jobs, job_rx) = channel::<Job>();
    let (done_tx, done) = channel::<Done>();
    let engine = shared.engine.clone();
    std::thread::spawn(move || worker(job_rx, done_tx, engine));
    // Requests not yet answered, oldest (the one rendering) first.
    let mut in_flight: VecDeque<(u64, Arc<Progress>)> = VecDeque::new();
    let mut last_progress = Instant::now();
    let mut last_request = Instant::now();
    let mut last_sent = Instant::now();
    loop {
        if last_sent.elapsed() >= KEEPALIVE {
            send(&mut socket, &ServerMsg::Keepalive)?;
            last_sent = Instant::now();
        }
        if in_flight.is_empty() && shared.idle_timeout.is_some_and(|t| last_request.elapsed() > t) {
            let reason = "idle for too long; reconnect to continue";
            let _ = send(&mut socket, &ServerMsg::Error { id: None, message: reason.into() });
            let close = tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Away,
                reason: reason.into(),
            };
            let _ = socket.close(Some(close));
            let _ = socket.flush();
            return Ok(());
        }
        loop {
            match done.try_recv() {
                Ok(Done::Frame(header, blocks)) => {
                    in_flight.retain(|(id, _)| *id != header.id);
                    send(&mut socket, &ServerMsg::Frame(*header))?;
                    for block in blocks {
                        socket.send(Message::binary(block)).map_err(|e| e.to_string())?;
                    }
                    last_sent = Instant::now();
                }
                Ok(Done::Error(id, message)) => {
                    in_flight.retain(|(i, _)| *i != id);
                    send(&mut socket, &ServerMsg::Error { id: Some(id), message })?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err("render worker stopped".into()),
            }
        }
        if let Some((id, progress)) = in_flight.front()
            && last_progress.elapsed() >= Duration::from_millis(100)
        {
            let (stage, fraction) = progress.snapshot();
            send(&mut socket, &ServerMsg::Progress { id: *id, stage, fraction })?;
            last_progress = Instant::now();
            last_sent = last_progress;
        }
        let text = match socket.read() {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(e) => return Err(e.to_string()),
        };
        last_request = Instant::now();
        match serde_json::from_str::<ClientMsg>(&text) {
            Ok(ClientMsg::Render { id, view, settings, queue }) => {
                if !queue {
                    for (_, p) in in_flight.drain(..) {
                        p.cancel();
                    }
                }
                let parsed = view.to_view().and_then(|v| Ok((v, settings.to_settings(shared.max_pixels)?)));
                match parsed {
                    Ok((mut view, settings)) => {
                        view.ensure_precision(settings.height);
                        let progress = Arc::new(Progress::default());
                        in_flight.push_back((id, progress.clone()));
                        last_progress = Instant::now();
                        jobs.send(Job { id, view, settings, progress }).map_err(|e| e.to_string())?;
                    }
                    Err(message) => send(&mut socket, &ServerMsg::Error { id: Some(id), message })?,
                }
            }
            Ok(ClientMsg::Cancel { id }) => {
                if let Some(i) = in_flight.iter().position(|(i, _)| *i == id) {
                    in_flight.remove(i).unwrap().1.cancel();
                }
            }
            Err(e) => send(&mut socket, &ServerMsg::Error { id: None, message: format!("bad request: {e}") })?,
        }
    }
    for (_, p) in in_flight {
        p.cancel();
    }
    Ok(())
}

/// Renders requests one at a time, in order, skipping those cancelled (by
/// a newer request that was not queued) before their turn.
fn worker(jobs: Receiver<Job>, done: Sender<Done>, engine: Option<Arc<dyn PixelEngine>>) {
    let mut renderer = Renderer::new();
    renderer.set_engine(engine);
    while let Ok(job) = jobs.recv() {
        if job.progress.cancelled() {
            continue;
        }
        let started = Instant::now();
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            renderer.render(&job.view, &job.settings, &job.progress)
        }));
        let render_s = started.elapsed().as_secs_f64();
        let outcome = match rendered {
            Ok(Some(frame)) => {
                let started = Instant::now();
                let blocks = codec::encode(&frame);
                let header = FrameHeader {
                    id: job.id,
                    width: frame.width,
                    height: frame.height,
                    maxiter: frame.maxiter,
                    de: !frame.de.is_empty(),
                    blocks: blocks.len() as u32,
                    bytes: blocks.iter().map(|b| b.len() as u64).sum(),
                    stats: StatsMsg::from(&frame.stats),
                    render_s,
                    encode_s: started.elapsed().as_secs_f64(),
                };
                Done::Frame(Box::new(header), blocks)
            }
            Ok(None) => continue,
            Err(_) => {
                // The renderer's caches may be inconsistent after a panic.
                renderer = Renderer::new();
                renderer.set_engine(None);
                Done::Error(job.id, "the server failed to render this view".into())
            }
        };
        if done.send(outcome).is_err() {
            return;
        }
    }
}
