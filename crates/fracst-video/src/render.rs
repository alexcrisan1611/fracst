//! Rendering a project's keyframes, deepest first, on this computer or on a
//! remote `fracst-server`. Each keyframe is saved as soon as it is done, so a job
//! that stops (cancelled, paused, crashed, or cut off by a remote's usage
//! limit) resumes from the first one missing.
//!
//! Remotely, requests are queued two deep when the server supports it, so
//! the GPU renders one keyframe while the previous one downloads. A dropped
//! connection is retried once; if that fails too (a gateway refusing a key
//! over its limit, say) the job pauses with the reason.

use crate::keyframe;
use crate::project::Project;
use fracst_core::{Progress, Renderer, View};
use fracst_remote::Client;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Where keyframes are rendered.
#[derive(Clone, Debug, PartialEq)]
pub enum Engine {
    Local,
    Remote { url: String, key: Option<String> },
}

/// Stops a job from another thread, and shows how far the keyframe in
/// flight has got.
#[derive(Debug, Default)]
pub struct Control {
    pub progress: Progress,
    pause: AtomicBool,
}

impl Control {
    /// Stop after saving what is done, to resume later.
    pub fn pause(&self) {
        self.pause.store(true, Ordering::Relaxed);
        self.progress.cancel();
    }

    pub fn cancel(&self) {
        self.progress.cancel();
    }

    pub fn stopped(&self) -> bool {
        self.progress.cancelled()
    }

    fn outcome(&self) -> Outcome {
        if self.pause.load(Ordering::Relaxed) { Outcome::Paused("paused".into()) } else { Outcome::Cancelled }
    }
}

pub enum Event {
    /// Connecting, or a gateway's report while it starts a GPU.
    Status(String),
    /// Rendering on this remote engine.
    Connected(String),
    /// Keyframe `index` is saved: `done` of `total` are.
    Keyframe { index: usize, done: usize, total: usize, seconds: f64 },
    /// After the first keyframe of a job: the time the rest will take at
    /// most, from how long that one took (the deepest are the slowest).
    Estimate { first: f64, remaining: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Every keyframe is rendered.
    Finished,
    /// Rendered as many as asked for.
    Limit,
    Cancelled,
    /// Stopped, for this reason; the job can resume.
    Paused(String),
}

/// Render the missing keyframes (at most `limit` of them).
pub fn render_keyframes(
    project: &Project,
    engine: &Engine,
    control: &Control,
    limit: Option<usize>,
    on: &mut dyn FnMut(Event),
) -> Result<Outcome, String> {
    let plan = project.plan();
    let missing: Vec<usize> = (0..plan.count).filter(|&k| !project.has_keyframe(k)).collect();
    let todo: Vec<usize> = missing.iter().copied().take(limit.unwrap_or(usize::MAX)).collect();
    if todo.is_empty() {
        return Ok(Outcome::Finished);
    }
    let all_done = todo.len() == missing.len();
    let settings = project.spec.keyframes.settings();
    let mut view = project.location.view.clone();
    view.ensure_precision(settings.height);
    // Every keyframe starts from the location's iteration limit, and
    // automatic iterations raise it where needed. Carrying the limit of the
    // keyframe before outwards would be wrong: deep down near a minibrot it
    // can be millions, and further out, where far fewer suffice, interior
    // pixels that are not detected as such would run to it.
    let view_for = |k: usize| View { zoom: plan.zoom(k), ..view.clone() };
    let total = plan.count;
    let mut done = total - missing.len();
    let mut first: Option<f64> = None;
    let mut saved = |k: usize, frame: &fracst_core::Frame, seconds: f64, on: &mut dyn FnMut(Event)| {
        keyframe::save(&project.keyframe_path(k), frame, plan.zoom(k))?;
        done += 1;
        on(Event::Keyframe { index: k, done, total, seconds });
        if first.is_none() {
            first = Some(seconds);
            let rest = todo.len() - 1;
            on(Event::Estimate { first: seconds, remaining: seconds * rest as f64 });
        }
        Ok::<_, String>(())
    };
    let finished = if all_done { Outcome::Finished } else { Outcome::Limit };

    match engine {
        Engine::Local => {
            let mut renderer = Renderer::new();
            for &k in &todo {
                let started = Instant::now();
                let Some(frame) = renderer.render(&view_for(k), &settings, &control.progress) else {
                    return Ok(control.outcome());
                };
                saved(k, &frame, started.elapsed().as_secs_f64(), on)?;
            }
            Ok(finished)
        }
        Engine::Remote { url, key } => {
            let mut client: Option<Client> = None;
            // Keyframes asked for and not yet received, oldest first.
            let mut outstanding: VecDeque<(usize, u64, Instant)> = VecDeque::new();
            let (mut next, mut received) = (0, 0);
            let mut failures = 0;
            let mut last_got: Option<Instant> = None;
            while received < todo.len() {
                if control.stopped() {
                    return Ok(control.outcome());
                }
                if client.is_none() {
                    on(Event::Status("Connecting…".into()));
                    let mut status = |m: &str| on(Event::Status(m.to_string()));
                    match Client::connect_with_status(url, key.as_deref(), &mut status) {
                        Ok(c) => {
                            on(Event::Connected(c.engine.clone()));
                            client = Some(c);
                            last_got = None;
                        }
                        Err(_) if control.stopped() => return Ok(control.outcome()),
                        Err(e) => return Ok(Outcome::Paused(e)),
                    }
                }
                let c = client.as_mut().unwrap();
                let depth = if c.can_queue() { 2 } else { 1 };
                let mut result = Ok(());
                while next < todo.len() && outstanding.len() < depth {
                    let k = todo[next];
                    match c.submit(&view_for(k), &settings, depth > 1) {
                        Ok(id) => outstanding.push_back((k, id, Instant::now())),
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                    next += 1;
                }
                let got = match result {
                    Ok(()) => {
                        let &(_, id, _) = outstanding.front().unwrap();
                        c.wait(id, &control.progress)
                    }
                    Err(e) => Err(e),
                };
                match got {
                    Ok(Some(frame)) => {
                        let (k, _, sent) = outstanding.pop_front().unwrap();
                        // Queued requests overlap, so count from the frame before.
                        let seconds = last_got.unwrap_or(sent).elapsed().as_secs_f64();
                        last_got = Some(Instant::now());
                        saved(k, &frame, seconds, on)?;
                        received += 1;
                        failures = 0;
                    }
                    Ok(None) => return Ok(control.outcome()),
                    Err(e) => {
                        // Start again from the oldest keyframe not received.
                        client = None;
                        outstanding.clear();
                        next = received;
                        failures += 1;
                        if failures >= 2 {
                            return Ok(Outcome::Paused(e));
                        }
                        on(Event::Status(format!("{e}; reconnecting…")));
                    }
                }
            }
            Ok(finished)
        }
    }
}
