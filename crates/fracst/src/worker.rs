//! Background thread that owns the renderer (and its reference/BLA cache).
//! The UI submits tasks; a newer task cancels the one in flight.

use fracst_core::{Colouring, FloatExp, Frame, Progress, Renderer, Settings, View, image, kfr, newton};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

pub enum Task {
    /// Render for display; `preview` first renders a quarter-resolution pass.
    Render { view: View, settings: Settings, preview: bool },
    /// Find the minibrot nearest the view centre and zoom to it.
    FindMinibrot { view: View, height: u32, fill: f64 },
    /// Render at another size and save a PNG with the location embedded.
    Export { view: View, settings: Settings, colouring: Colouring, supersample: u32, path: PathBuf },
}

pub enum Outcome {
    Frame { view: View, frame: Box<Frame>, preview: bool },
    Minibrot(Result<(View, u64), String>),
    Exported(Result<(PathBuf, Duration), String>),
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

pub struct Worker {
    tx: Sender<Request>,
    pub rx: Receiver<Message>,
    next_id: u64,
    current: Option<(u64, Arc<Progress>)>,
}

impl Worker {
    pub fn spawn(ctx: eframe::egui::Context) -> Self {
        let (tx, jobs) = channel::<Request>();
        let (done, rx) = channel::<Message>();
        std::thread::Builder::new()
            .name("fracst-render".into())
            .spawn(move || run(jobs, done, ctx))
            .expect("spawn render thread");
        Worker { tx, rx, next_id: 1, current: None }
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
}

fn run(jobs: Receiver<Request>, done: Sender<Message>, ctx: eframe::egui::Context) {
    let mut renderer = Renderer::new();
    let mut pending: Option<Request> = None;
    loop {
        let req = match pending.take() {
            Some(r) => r,
            None => match jobs.recv() {
                Ok(r) => r,
                Err(_) => return,
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
                if preview {
                    let small = Settings {
                        width: (settings.width / 4).max(1),
                        height: (settings.height / 4).max(1),
                        ..settings.clone()
                    };
                    if let Some(frame) = renderer.render(&view, &small, &req.progress) {
                        send(Outcome::Frame { view: view.clone(), frame: Box::new(frame), preview: true });
                    }
                    // A newer request makes the full-resolution pass pointless.
                    if let Ok(newer) = jobs.try_recv() {
                        pending = Some(newer);
                        continue;
                    }
                }
                if let Some(frame) = renderer.render(&view, &settings, &req.progress) {
                    send(Outcome::Frame { view, frame: Box::new(frame), preview: false });
                }
            }
            Task::FindMinibrot { view, height, fill } => {
                let radius = view.pixel_spacing(height) * FloatExp::from(height as f64 / 2.0);
                let maxperiod = view.maxiter.max(1000);
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
                let result = match renderer.render(&view, &big, &req.progress) {
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
