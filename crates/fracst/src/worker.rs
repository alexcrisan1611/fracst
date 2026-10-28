//! Background thread that owns the renderer (and its reference/BLA cache).
//! The UI submits tasks; a newer task cancels the one in flight.
//!
//! Renders can also go to a remote `fracst-server` (a cloud GPU, say). The
//! connection is made on a separate thread, so rendering carries on locally
//! until the server is ready, and falls back to local if it goes away.

use fracst_core::{Colouring, FloatExp, Frame, Progress, Renderer, Settings, View, image, kfr, newton};
use fracst_remote::Client;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

pub enum Task {
    /// Render for display; `preview` first renders a quarter-resolution pass.
    Render { view: View, settings: Settings, preview: bool },
    /// Find the minibrot nearest the view centre and zoom to it.
    FindMinibrot { view: View, height: u32, fill: f64 },
    /// Render at another size and save a PNG with the location embedded.
    Export { view: View, settings: Settings, colouring: Colouring, supersample: u32, path: PathBuf },
}

/// Where to render remotely.
#[derive(Clone)]
pub struct RemoteConfig {
    /// `ws://` or `wss://` address of a fracst server.
    pub url: String,
    /// fal.ai key or server token (may be empty).
    pub key: String,
}

#[derive(Clone, Debug)]
pub enum RemoteStatus {
    Off,
    /// With the latest report, e.g. from a gateway starting a GPU.
    Connecting(String),
    /// Rendering remotely on this engine.
    Connected(String),
    /// Disconnected after a while without renders, so the GPU can shut
    /// down; the next render reconnects.
    Paused,
    Failed(String),
}

pub enum Outcome {
    /// `round_trip` is set for frames rendered remotely.
    Frame {
        view: View,
        frame: Box<Frame>,
        preview: bool,
        round_trip: Option<Duration>,
    },
    Minibrot(Result<(View, u64), String>),
    Exported(Result<(PathBuf, Duration), String>),
    /// The remote connection changed; `generation` identifies the request
    /// (see [`Worker::connect_remote`]) so stale reports can be ignored.
    Remote {
        generation: u64,
        status: RemoteStatus,
    },
}

pub struct Message {
    pub id: u64,
    pub outcome: Outcome,
}

struct Request {
    id: u64,
    task: Task,
    progress: Arc<Progress>,
}

/// Remote connection changes, for the render thread.
enum RemoteMsg {
    /// Stop rendering remotely; only clients of this generation count from now on.
    Reset(u64),
    Client(u64, Box<Client>),
}

pub struct Worker {
    tx: Sender<Request>,
    pub rx: Receiver<Message>,
    next_id: u64,
    current: Option<(u64, Arc<Progress>)>,
    remote_tx: Sender<RemoteMsg>,
    done_tx: Sender<Message>,
    ctx: eframe::egui::Context,
    remote_generation: u64,
}

impl Worker {
    pub fn spawn(ctx: eframe::egui::Context) -> Self {
        let (tx, jobs) = channel::<Request>();
        let (done_tx, rx) = channel::<Message>();
        let (remote_tx, remote_rx) = channel::<RemoteMsg>();
        let (done, thread_ctx) = (done_tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("fracst-render".into())
            .spawn(move || run(jobs, remote_rx, done, thread_ctx))
            .expect("spawn render thread");
        Worker { tx, rx, next_id: 1, current: None, remote_tx, done_tx, ctx, remote_generation: 0 }
    }

    /// Submit a task, cancelling the previous one. Returns its id.
    pub fn submit(&mut self, task: Task) -> u64 {
        self.cancel();
        let id = self.next_id;
        self.next_id += 1;
        let progress = Arc::new(Progress::default());
        self.current = Some((id, progress.clone()));
        let _ = self.tx.send(Request { id, task, progress });
        id
    }

    pub fn cancel(&mut self) {
        if let Some((_, p)) = self.current.take() {
            p.cancel();
        }
    }

    /// The task in flight, if any, and its progress.
    pub fn current(&self) -> Option<&(u64, Arc<Progress>)> {
        self.current.as_ref()
    }

    pub fn finished(&mut self, id: u64) {
        if self.current.as_ref().is_some_and(|(c, _)| *c == id) {
            self.current = None;
        }
    }

    /// Start connecting to a remote server in the background; renders stay
    /// local until it answers. Returns the generation its reports carry.
    pub fn connect_remote(&mut self, config: RemoteConfig) -> u64 {
        let generation = self.disconnect_remote();
        let (remote, done, ctx) = (self.remote_tx.clone(), self.done_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let report = |status| {
                let _ = done.send(Message { id: 0, outcome: Outcome::Remote { generation, status } });
                ctx.request_repaint();
            };
            let mut progress = |message: &str| report(RemoteStatus::Connecting(message.to_string()));
            let status = match Client::connect_with_status(&config.url, Some(&config.key), &mut progress) {
                Ok(client) => {
                    let engine = format!("{} · fracst {}", client.engine, client.version);
                    let _ = remote.send(RemoteMsg::Client(generation, Box::new(client)));
                    RemoteStatus::Connected(engine)
                }
                Err(e) => RemoteStatus::Failed(e),
            };
            report(status);
        });
        generation
    }

    /// Render locally from the next task on. Returns the new generation.
    pub fn disconnect_remote(&mut self) -> u64 {
        self.remote_generation += 1;
        let _ = self.remote_tx.send(RemoteMsg::Reset(self.remote_generation));
        self.remote_generation
    }
}

/// The render thread's renderers: always the local one, and a remote client while connected.
struct Backend {
    local: Renderer,
    remote: Option<Client>,
    generation: u64,
    remote_rx: Receiver<RemoteMsg>,
}

impl Backend {
    fn poll_remote(&mut self) {
        while let Ok(msg) = self.remote_rx.try_recv() {
            match msg {
                RemoteMsg::Reset(g) => {
                    self.generation = g;
                    self.remote = None;
                }
                RemoteMsg::Client(g, client) if g == self.generation => self.remote = Some(*client),
                RemoteMsg::Client(..) => {}
            }
        }
    }

    /// Render remotely if connected (falling back to local if that fails),
    /// otherwise locally. Also returns the round trip for remote frames.
    fn render(
        &mut self,
        view: &View,
        settings: &Settings,
        progress: &Progress,
        report: &dyn Fn(Outcome),
    ) -> (Option<Frame>, Option<Duration>) {
        self.poll_remote();
        if let Some(client) = &mut self.remote {
            let started = Instant::now();
            match client.render(view, settings, progress) {
                Ok(frame) => return (frame, Some(started.elapsed())),
                Err(e) => {
                    self.remote = None;
                    let status = RemoteStatus::Failed(format!("{e} (rendering on this computer)"));
                    report(Outcome::Remote { generation: self.generation, status });
                }
            }
        }
        (self.local.render(view, settings, progress), None)
    }
}

fn run(jobs: Receiver<Request>, remote_rx: Receiver<RemoteMsg>, done: Sender<Message>, ctx: eframe::egui::Context) {
    let mut backend = Backend { local: Renderer::new(), remote: None, generation: 0, remote_rx };
    let mut pending: Option<Request> = None;
    loop {
        let req = match pending.take() {
            Some(r) => r,
            // Wake up now and then to act on disconnects, so a remote GPU
            // isn't kept up by a connection nobody uses.
            None => match jobs.recv_timeout(Duration::from_millis(250)) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => {
                    backend.poll_remote();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        // Skip straight to the newest request.
        let mut req = req;
        loop {
            match jobs.try_recv() {
                Ok(newer) => req = newer,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if req.progress.cancelled() {
            continue;
        }
        let send = |outcome| {
            let _ = done.send(Message { id: req.id, outcome });
            ctx.request_repaint();
        };
        match req.task {
            Task::Render { view, settings, preview } => {
                let mut hint = None;
                if preview {
                    let small = Settings {
                        width: (settings.width / 4).max(1),
                        height: (settings.height / 4).max(1),
                        ..settings.clone()
                    };
                    if let (Some(frame), round_trip) = backend.render(&view, &small, &req.progress, &send) {
                        // With automatic iterations the full pass can start where the preview ended.
                        hint = settings.auto_iterations.then_some(frame.maxiter);
                        send(Outcome::Frame { view: view.clone(), frame: Box::new(frame), preview: true, round_trip });
                    }
                    // A newer request makes the full-resolution pass pointless.
                    if let Ok(newer) = jobs.try_recv() {
                        pending = Some(newer);
                        continue;
                    }
                }
                let full = View { maxiter: hint.unwrap_or(view.maxiter), ..view.clone() };
                if let (Some(frame), round_trip) = backend.render(&full, &settings, &req.progress, &send) {
                    send(Outcome::Frame { view, frame: Box::new(frame), preview: false, round_trip });
                }
            }
            Task::FindMinibrot { view, height, fill } => {
                let radius = view.pixel_spacing(height) * FloatExp::from(height as f64 / 2.0);
                // The probe orbit stops when it escapes, so a generous bound is cheap.
                let maxperiod = view.maxiter.max(1 << 24);
                let result = match newton::find_minibrot(&view.re, &view.im, radius, maxperiod, &req.progress) {
                    Some(n) => {
                        let mut v = view.clone();
                        v.re = n.re;
                        v.im = n.im;
                        v.zoom = FloatExp::from(fill) / n.size;
                        v.ensure_precision(height);
                        Ok((v, n.period))
                    }
                    None if req.progress.cancelled() => continue,
                    None => Err("No minibrot found near the centre of the view.".to_string()),
                };
                send(Outcome::Minibrot(result));
            }
            Task::Export { mut view, settings, colouring, supersample, path } => {
                let started = std::time::Instant::now();
                let ss = supersample.max(1);
                let big = Settings { width: settings.width * ss, height: settings.height * ss, ..settings.clone() };
                view.ensure_precision(big.height);
                let result = match backend.render(&view, &big, &req.progress, &send).0 {
                    Some(frame) => {
                        let rgba = image::downscale(&colouring.apply(&frame), big.width, big.height, ss);
                        let comment = kfr::to_string(&view, &colouring);
                        image::save_png(&path, &rgba, settings.width, settings.height, Some(&comment))
                            .map(|_| (path, started.elapsed()))
                    }
                    None => continue,
                };
                send(Outcome::Exported(result));
            }
        }
    }
}
