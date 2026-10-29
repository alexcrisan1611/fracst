//! The Video window: set up a zoom video into the current location, render
//! its keyframes in the background (here or on the remote GPU) while the
//! explorer stays usable, scrub through a preview, and encode it.
//!
//! The work happens in `fracst_video`; jobs run on their own threads, in
//! their own rayon pool so that the explorer's renders are not stuck
//! behind them, and report back over a channel.

use eframe::egui::{self, Color32, ComboBox, DragValue, Slider, TextureHandle, TextureOptions};
use fracst_core::Colouring;
use fracst_core::kfr::Location;
use fracst_video::{
    Codec, ColourKey, Control, Engine, Event, Length, Outcome, Project, Quality, VideoSpec, Written, ffmpeg,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

/// Longest side of the preview image.
const PREVIEW_SIZE: u32 = 480;

enum Msg {
    Status(String),
    Keyframe {
        done: usize,
        total: usize,
    },
    Estimate {
        first: f64,
        remaining: f64,
    },
    Frames {
        done: usize,
        total: usize,
    },
    Download {
        got: u64,
        total: u64,
    },
    /// A job ended, with a message for the status line.
    Done(Result<String, String>),
    /// A job paused (it can resume), for this reason.
    Paused(String),
    Preview {
        generation: u64,
        t: f64,
        total: f64,
        image: Result<egui::ColorImage, String>,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Estimate,
    Keyframes,
    Encode,
    Download,
}

struct Job {
    kind: JobKind,
    control: Arc<Control>,
    /// For downloads.
    cancel: Arc<AtomicBool>,
    started: Instant,
    progress: (usize, usize),
    /// Progress when this run started, for the time left.
    progress_at_start: usize,
}

/// A request to the preview thread.
struct PreviewReq {
    dir: PathBuf,
    generation: u64,
    t: f64,
}

pub struct VideoPanel {
    pub open: bool,
    spec: VideoSpec,
    /// Where a new project zooms to (the explorer's location when opened).
    location: Option<Location>,
    project: Option<Project>,
    remote: bool,
    job: Option<Job>,
    status: String,
    paused: Option<String>,
    estimate: Option<(f64, f64)>,
    /// Asking whether to download ffmpeg before encoding.
    ask_ffmpeg: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    preview_tx: Sender<PreviewReq>,
    /// Bumped whenever the project on disk changes, so previews follow.
    generation: u64,
    preview_t: f64,
    preview_total: f64,
    /// Generation and time of the preview requested last.
    preview_asked: Option<(u64, f64)>,
    preview: Option<TextureHandle>,
    preview_error: Option<String>,
}

impl VideoPanel {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let (preview_tx, preview_rx) = channel();
        let (done, thread_ctx) = (tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("fracst-preview".into())
            .spawn(move || preview_thread(preview_rx, done, thread_ctx))
            .expect("spawn preview thread");
        VideoPanel {
            open: false,
            spec: VideoSpec::default(),
            location: None,
            project: None,
            remote: false,
            job: None,
            status: String::new(),
            paused: None,
            estimate: None,
            ask_ffmpeg: false,
            tx,
            rx,
            ctx,
            preview_tx,
            generation: 0,
            preview_t: 0.0,
            preview_total: 0.0,
            preview_asked: None,
            preview: None,
            preview_error: None,
        }
    }

    /// The location of the open project.
    pub fn project_location(&self) -> Option<&Location> {
        self.project.as_ref().map(|p| &p.location)
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Set up a new video into this location.
    pub fn start_new(&mut self, location: Location) {
        if self.busy() {
            self.open = true;
            self.status = "Finish or cancel the current video job first.".into();
            return;
        }
        self.location = Some(location);
        self.project = None;
        self.reset_state();
        self.open = true;
    }

    /// Open an existing project folder.
    pub fn open_project(&mut self, dir: &Path) -> Result<(), String> {
        if self.busy() {
            return Err("finish or cancel the current video job first".into());
        }
        let project = Project::open(dir)?;
        self.spec = project.spec.clone();
        self.location = Some(project.location.clone());
        self.project = Some(project);
        self.reset_state();
        self.status = format!("Opened {}.", dir.display());
        self.open = true;
        Ok(())
    }

    fn reset_state(&mut self) {
        self.paused = None;
        self.estimate = None;
        self.ask_ffmpeg = false;
        self.preview = None;
        self.preview_error = None;
        self.preview_asked = None;
        self.preview_t = 0.0;
        self.generation += 1;
        self.status.clear();
    }

    fn keyframes_locked(&self) -> bool {
        self.project.as_ref().is_some_and(|p| p.keyframes_done() > 0)
    }

    /// Save edits to an existing project's settings.
    fn save_spec(&mut self) {
        let Some(project) = &mut self.project else { return };
        if project.spec == self.spec {
            return;
        }
        match project.update(self.spec.clone()) {
            Ok(()) => {
                self.generation += 1;
                self.preview_asked = None;
            }
            Err(e) => {
                self.status = e;
                self.spec = project.spec.clone();
            }
        }
    }

    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(s) => self.status = s,
                Msg::Keyframe { done, total } | Msg::Frames { done, total } => {
                    if let Some(job) = &mut self.job {
                        job.progress = (done, total);
                    }
                    // New keyframes show up in the preview.
                    self.preview_asked = None;
                }
                Msg::Download { got, total } => {
                    if let Some(job) = &mut self.job {
                        job.progress = (got as usize, total as usize);
                    }
                }
                Msg::Estimate { first, remaining } => self.estimate = Some((first, remaining)),
                Msg::Done(result) => {
                    let kind = self.job.take().map(|j| j.kind);
                    match result {
                        Ok(s) => {
                            self.status = s;
                            self.paused = None;
                            if kind == Some(JobKind::Download) {
                                self.encode(ffmpeg::find());
                            }
                        }
                        Err(e) => self.status = e,
                    }
                    self.preview_asked = None;
                }
                Msg::Paused(reason) => {
                    self.job = None;
                    self.paused = Some(reason);
                    self.preview_asked = None;
                }
                Msg::Preview { generation, t, total, image } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.preview_total = total;
                    let _ = t;
                    match image {
                        Ok(img) => {
                            self.preview_error = None;
                            match &mut self.preview {
                                Some(tex) => tex.set(img, TextureOptions::LINEAR),
                                None => {
                                    self.preview =
                                        Some(self.ctx.load_texture("video-preview", img, TextureOptions::LINEAR))
                                }
                            }
                        }
                        Err(e) => self.preview_error = Some(e),
                    }
                }
            }
        }
    }

    fn start_job(
        &mut self,
        kind: JobKind,
        work: impl FnOnce(Arc<Control>, Arc<AtomicBool>, Sender<Msg>) + Send + 'static,
    ) {
        let control = Arc::new(Control::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, k, tx, ctx) = (control.clone(), cancel.clone(), self.tx.clone(), self.ctx.clone());
        let progress_at_start = self.project.as_ref().map_or(0, |p| p.keyframes_done());
        std::thread::Builder::new()
            .name("fracst-video".into())
            .spawn(move || {
                let notify = Notifier { tx, ctx };
                let pool = rayon::ThreadPoolBuilder::new().thread_name(|i| format!("fracst-video-{i}")).build();
                match pool {
                    Ok(pool) => pool.install(|| work(c, k, notify.tx.clone())),
                    Err(e) => notify.send(Msg::Done(Err(e.to_string()))),
                }
                notify.ctx.request_repaint();
            })
            .expect("spawn video thread");
        self.job = Some(Job {
            kind,
            control,
            cancel,
            started: Instant::now(),
            progress: (0, 0),
            progress_at_start: if kind == JobKind::Keyframes { progress_at_start } else { 0 },
        });
        self.paused = None;
    }

    /// Create the project if there is none yet, in a folder the user picks.
    fn ensure_project(&mut self) -> bool {
        if self.project.is_some() {
            self.save_spec();
            return true;
        }
        let Some(location) = self.location.clone() else { return false };
        let Some(dir) = rfd::FileDialog::new().set_title("Folder for the video project").pick_folder() else {
            return false;
        };
        // An empty folder is used as it is; otherwise a new one goes inside it.
        let empty = std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(true);
        let dir = if empty || Project::exists(&dir) { dir } else { dir.join("fracst-video") };
        if Project::exists(&dir) {
            if let Err(e) = self.open_project(&dir) {
                self.status = e;
                return false;
            }
            self.status = format!("{} already held a project; opened it.", dir.display());
            return true;
        }
        match Project::create(&dir, location, self.spec.clone()) {
            Ok(p) => {
                self.status = format!("Project in {}.", dir.display());
                self.project = Some(p);
                self.generation += 1;
                true
            }
            Err(e) => {
                self.status = e;
                false
            }
        }
    }

    fn render_keyframes(&mut self, remote: Option<(String, String)>, limit: Option<usize>) {
        if !self.ensure_project() {
            return;
        }
        let dir = self.project.as_ref().unwrap().dir.clone();
        let engine = match remote.filter(|_| self.remote) {
            Some((url, key)) => Engine::Remote { url, key: Some(key).filter(|k| !k.is_empty()) },
            None => Engine::Local,
        };
        let kind = if limit.is_some() { JobKind::Estimate } else { JobKind::Keyframes };
        self.start_job(kind, move |control, _, tx| {
            let result = (|| {
                let project = Project::open(&dir)?;
                let mut on = |e: Event| {
                    let _ = tx.send(match e {
                        Event::Status(s) => Msg::Status(s),
                        Event::Connected(engine) => Msg::Status(format!("Rendering keyframes on {engine}")),
                        Event::Keyframe { done, total, .. } => Msg::Keyframe { done, total },
                        Event::Estimate { first, remaining } => Msg::Estimate { first, remaining },
                    });
                };
                on(Event::Status("Rendering keyframes…".into()));
                fracst_video::render_keyframes(&project, &engine, &control, limit, &mut on)
            })();
            let _ = tx.send(match result {
                Ok(Outcome::Finished) => Msg::Done(Ok("All keyframes are rendered.".into())),
                Ok(Outcome::Limit) => Msg::Done(Ok("Rendered a keyframe for the estimate.".into())),
                Ok(Outcome::Cancelled) => Msg::Done(Ok("Stopped. Rendered keyframes are kept.".into())),
                Ok(Outcome::Paused(reason)) => Msg::Paused(reason),
                Err(e) => Msg::Done(Err(e)),
            });
        });
    }

    fn encode(&mut self, ff: Option<PathBuf>) {
        if !self.ensure_project() {
            return;
        }
        self.ask_ffmpeg = false;
        let dir = self.project.as_ref().unwrap().dir.clone();
        self.start_job(JobKind::Encode, move |control, _, tx| {
            let result = (|| {
                let project = Project::open(&dir)?;
                let _ = tx.send(Msg::Status(if ff.is_some() { "Encoding…" } else { "Writing PNG frames…" }.into()));
                let mut on = |done, total| {
                    let _ = tx.send(Msg::Frames { done, total });
                };
                fracst_video::encode(&project, ff.as_deref(), &control, &mut on)
            })();
            let _ = tx.send(Msg::Done(match result {
                Ok((Outcome::Finished, Written::Video(p))) => Ok(format!("Wrote {}.", p.display())),
                Ok((Outcome::Finished, Written::Frames(d))) => Ok(format!("Wrote PNG frames to {}.", d.display())),
                Ok(_) => Ok("Encoding cancelled.".into()),
                Err(e) => Err(e),
            }));
        });
    }

    fn download_ffmpeg(&mut self) {
        self.ask_ffmpeg = false;
        self.start_job(JobKind::Download, move |_, cancel, tx| {
            let _ = tx.send(Msg::Status("Downloading ffmpeg…".into()));
            let progress_tx = tx.clone();
            let mut progress = |got, total| {
                let _ = progress_tx.send(Msg::Download { got, total });
            };
            let _ = tx.send(Msg::Done(
                ffmpeg::download(&mut progress, &cancel).map(|p| format!("ffmpeg installed at {}.", p.display())),
            ));
        });
    }

    pub fn ui(&mut self, ctx: &egui::Context, remote: Option<(String, String)>, colouring: &Colouring) {
        self.poll();
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Video")
            .open(&mut open)
            .default_width(460.0)
            .default_height(ctx.content_rect().height() * 0.85)
            .vscroll(true)
            .show(ctx, |ui| self.contents(ui, remote, colouring));
        self.open = open;
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn contents(&mut self, ui: &mut egui::Ui, remote: Option<(String, String)>, colouring: &Colouring) {
        let Some(location) = &self.location else {
            ui.label("Open a location first.");
            return;
        };
        ui.label(format!("Zoom into {:.4}", location.view.zoom));
        match &self.project {
            Some(p) => {
                let plan = p.plan();
                ui.label(format!("Project: {}", p.dir.display()));
                ui.label(format!(
                    "{} of {} keyframes rendered ({:.1} zoom doublings)",
                    p.keyframes_done(),
                    plan.count,
                    plan.doublings()
                ));
            }
            None => {
                ui.label("Not saved yet: a project folder is chosen when rendering starts.");
            }
        }
        let busy = self.busy();
        let before = self.spec.clone();
        ui.add_enabled_ui(!busy, |ui| {
            egui::CollapsingHeader::new("Video").default_open(true).show(ui, |ui| self.video_ui(ui));
            egui::CollapsingHeader::new("Colour animation")
                .default_open(false)
                .show(ui, |ui| self.colour_ui(ui, colouring));
            egui::CollapsingHeader::new("Output").default_open(true).show(ui, |ui| self.output_ui(ui));
        });
        if self.spec != before {
            self.save_spec();
        }
        ui.separator();
        self.actions_ui(ui, remote);
        ui.separator();
        self.preview_ui(ui);
    }

    fn video_ui(&mut self, ui: &mut egui::Ui) {
        let locked = self.keyframes_locked();
        let s = &mut self.spec;
        egui::Grid::new("video-settings").num_columns(2).show(ui, |ui| {
            ui.label("Size");
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    let mut changed = ui.add(DragValue::new(&mut s.width).range(16..=7680).speed(2)).changed();
                    ui.label("×");
                    changed |= ui.add(DragValue::new(&mut s.height).range(16..=4320).speed(2)).changed();
                    ComboBox::from_id_salt("size-preset").selected_text("Preset").show_ui(ui, |ui| {
                        for (name, w, h) in [
                            ("720p", 1280, 720),
                            ("1080p", 1920, 1080),
                            ("1440p", 2560, 1440),
                            ("4K", 3840, 2160),
                            ("Square 1080", 1080, 1080),
                            ("Vertical 1080", 1080, 1920),
                        ] {
                            if ui.selectable_label(false, name).clicked() {
                                (s.width, s.height) = (w, h);
                                changed = true;
                            }
                        }
                    });
                    if changed {
                        s.width += s.width % 2;
                        s.height += s.height % 2;
                        s.size_keyframes();
                    }
                });
            })
            .response
            .on_disabled_hover_text("Fixed once keyframes are rendered");
            ui.end_row();

            ui.label("Quality");
            ui.add_enabled_ui(!locked, |ui| {
                ComboBox::from_id_salt("quality").selected_text(s.quality.name()).show_ui(ui, |ui| {
                    for q in Quality::ALL {
                        if ui.selectable_value(&mut s.quality, q, q.name()).changed() {
                            s.size_keyframes();
                        }
                    }
                })
                .response
                .on_hover_text("Draft: keyframes at the video's size. Good: 1.5×, Best: 2× (sharpest), with more samples per pixel");
            });
            ui.end_row();

            ui.label("Frame rate");
            ComboBox::from_id_salt("fps").selected_text(format!("{} fps", s.fps)).show_ui(ui, |ui| {
                for fps in [24.0, 25.0, 30.0, 50.0, 60.0] {
                    ui.selectable_value(&mut s.fps, fps, format!("{fps} fps"));
                }
            });
            ui.end_row();

            let t = &mut s.timing;
            ui.label("Length");
            ui.horizontal(|ui| {
                let (mut secs, mut per) = match t.length {
                    Length::Seconds(v) => (v, 1.0),
                    Length::PerDoubling(v) => (60.0, v),
                    Length::FitAudio => (60.0, 1.0),
                };
                ui.radio_value(&mut t.length, Length::PerDoubling(per), "per doubling");
                ui.radio_value(&mut t.length, Length::Seconds(secs), "total");
                let has_audio = s.encode.audio.is_some();
                ui.add_enabled(has_audio, egui::RadioButton::new(t.length == Length::FitAudio, "fit audio"))
                    .on_disabled_hover_text("Choose an audio track first")
                    .clicked()
                    .then(|| t.length = Length::FitAudio);
                match &mut t.length {
                    Length::PerDoubling(v) => {
                        if ui.add(DragValue::new(&mut per).range(0.05..=60.0).speed(0.01).suffix(" s")).changed() {
                            *v = per;
                        }
                    }
                    Length::Seconds(v) => {
                        if ui.add(DragValue::new(&mut secs).range(1.0..=36000.0).speed(0.5).suffix(" s")).changed() {
                            *v = secs;
                        }
                    }
                    Length::FitAudio => {}
                }
            });
            ui.end_row();

            ui.label("Speed up / slow down");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut t.ease_in).range(0.0..=60.0).speed(0.1).suffix(" s"))
                    .on_hover_text("Seconds to reach full speed at the start");
                ui.add(DragValue::new(&mut t.ease_out).range(0.0..=60.0).speed(0.1).suffix(" s"))
                    .on_hover_text("Seconds to slow down to a stop at the location");
            });
            ui.end_row();

            ui.label("Hold start / end");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut t.hold_start).range(0.0..=60.0).speed(0.1).suffix(" s"));
                ui.add(DragValue::new(&mut t.hold_end).range(0.0..=60.0).speed(0.1).suffix(" s"));
            });
            ui.end_row();
        });
    }

    fn colour_ui(&mut self, ui: &mut egui::Ui, current: &Colouring) {
        let c = &mut self.spec.colour;
        ui.add(Slider::new(&mut c.cycle, -200.0..=200.0).text("palette cycling / s"))
            .on_hover_text("Palette entries (of 1024) the colours move per second");
        ui.label("Colour keys: the colours at moments of the video, eased in between.");
        let total = self.preview_total;
        let mut remove = None;
        for (i, key) in c.keys.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut key.time).range(0.0..=36000.0).speed(0.1).prefix("at ").suffix(" s"));
                let num = |ui: &mut egui::Ui, v: &mut Option<f64>, label: &str, speed: f64| {
                    if let Some(x) = v {
                        ui.add(DragValue::new(x).speed(speed).prefix(label));
                    }
                };
                num(ui, &mut key.offset, "offset ", 1.0);
                if let Some(d) = &mut key.iter_div {
                    ui.add(DragValue::new(d).range(0.0001..=10000.0).speed(0.001).prefix("div "));
                }
                num(ui, &mut key.slope_angle, "angle ", 1.0);
                if ui.small_button("Set to current").on_hover_text("Take the explorer's colours").clicked() {
                    *key = key_from(current, key.time);
                }
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            c.keys.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button("Add key from current colours").clicked() {
                let time = if c.keys.is_empty() { 0.0 } else { self.preview_t.max(0.0) };
                c.keys.push(key_from(current, time));
            }
            if total > 0.0 {
                ui.label(format!("(at the preview's time, {:.1} s)", self.preview_t));
            }
        });
    }

    fn output_ui(&mut self, ui: &mut egui::Ui) {
        let e = &mut self.spec.encode;
        egui::Grid::new("video-output").num_columns(2).show(ui, |ui| {
            ui.label("Codec");
            ComboBox::from_id_salt("codec").selected_text(e.codec.name()).show_ui(ui, |ui| {
                for c in Codec::ALL {
                    if ui.selectable_value(&mut e.codec, c, c.name()).changed() {
                        let ext = c.extension();
                        e.output.set_extension(ext);
                    }
                }
            });
            ui.end_row();
            ui.label("File");
            ui.horizontal(|ui| {
                ui.label(e.output.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
                    .on_hover_text(e.output.display().to_string());
                if ui.small_button("Choose…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .add_filter("Video", &[e.codec.extension()])
                        .set_file_name(format!("fracst.{}", e.codec.extension()))
                        .save_file()
                {
                    e.output = p;
                }
            });
            ui.end_row();
            ui.label("Audio");
            ui.horizontal(|ui| {
                let name = e.audio.as_ref().and_then(|a| a.file_name()).map(|f| f.to_string_lossy().into_owned());
                ui.label(name.as_deref().unwrap_or("none"));
                if ui.small_button("Choose…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .add_filter("Audio", &["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus"])
                        .pick_file()
                {
                    e.audio = Some(p);
                }
                if e.audio.is_some() && ui.small_button("✕").clicked() {
                    e.audio = None;
                    if self.spec.timing.length == Length::FitAudio {
                        self.spec.timing.length = Length::PerDoubling(1.0);
                    }
                }
            });
            ui.end_row();
        });
    }

    fn actions_ui(&mut self, ui: &mut egui::Ui, remote: Option<(String, String)>) {
        let has_remote = remote.is_some();
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Render keyframes on");
                ui.radio_value(&mut self.remote, false, "this computer");
                ui.add_enabled(has_remote, egui::RadioButton::new(self.remote, "the remote GPU"))
                    .on_disabled_hover_text("Set a server under Remote GPU first")
                    .clicked()
                    .then(|| self.remote = true);
            });
        });
        if !has_remote {
            self.remote = false;
        }

        if let Some(job) = &self.job {
            let (done, total) = job.progress;
            let (label, frac) = match job.kind {
                JobKind::Download => (
                    format!("{:.0} of {:.0} MB", done as f64 / 1e6, total as f64 / 1e6),
                    done as f32 / total.max(1) as f32,
                ),
                JobKind::Encode => (format!("frame {done} of {total}"), done as f32 / total.max(1) as f32),
                JobKind::Keyframes | JobKind::Estimate => {
                    let (_, current) = job.control.progress.snapshot();
                    let frac = if total > 0 { (done as f64 + current) / total as f64 } else { current };
                    (format!("keyframe {} of {}", (done + 1).min(total.max(1)), total), frac as f32)
                }
            };
            let left = time_left(job);
            ui.add(egui::ProgressBar::new(frac).text(match left {
                Some(l) => format!("{label} · {l} left"),
                None => label,
            }));
            ui.horizontal(|ui| {
                if matches!(job.kind, JobKind::Keyframes) && ui.button("Pause").clicked() {
                    job.control.pause();
                }
                if ui.button("Cancel").clicked() {
                    job.control.cancel();
                    job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
        } else {
            let (done, count) = self.project.as_ref().map_or((0, 0), |p| (p.keyframes_done(), p.plan().count));
            let all = count > 0 && done == count;
            ui.horizontal(|ui| {
                if !all {
                    let label = if done > 0 { "Resume keyframes" } else { "Render keyframes" };
                    if ui.button(label).clicked() {
                        self.render_keyframes(remote.clone(), None);
                    }
                    if ui
                        .button("Estimate")
                        .on_hover_text("Render the next keyframe (the slowest are the deepest) and estimate the rest")
                        .clicked()
                    {
                        self.render_keyframes(remote.clone(), Some(1));
                    }
                }
                if ui.add_enabled(all, egui::Button::new("Encode video")).clicked() {
                    match ffmpeg::find() {
                        Some(ff) => self.encode(Some(ff)),
                        None => self.ask_ffmpeg = true,
                    }
                }
                if let Some(p) = &self.project
                    && ui.small_button("Open folder").clicked()
                {
                    open_in_file_manager(&p.dir);
                }
            });
        }
        if self.ask_ffmpeg && !self.busy() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label("ffmpeg is needed to make a video file, and none was found.");
                ui.horizontal(|ui| {
                    if let Some(info) = ffmpeg::download_info()
                        && ui
                            .button(format!("Download ffmpeg {} ({:.0} MB)", info.version, info.size as f64 / 1e6))
                            .on_hover_text(format!(
                                "From {}: {}\nChecked against a pinned SHA-256 and kept in fracst's data folder.",
                                info.source, info.url
                            ))
                            .clicked()
                    {
                        self.download_ffmpeg();
                    }
                    if ui
                        .button("Locate ffmpeg…")
                        .on_hover_text("Choose an ffmpeg you have; fracst remembers it")
                        .clicked()
                        && let Some(exe) = rfd::FileDialog::new().set_title("Choose ffmpeg").pick_file()
                    {
                        match ffmpeg::remember(&exe) {
                            Ok(()) => self.encode(Some(exe)),
                            Err(e) => self.status = e,
                        }
                    }
                    if ui.button("Write PNG frames instead").clicked() {
                        self.encode(None);
                    }
                    if ui.button("Not now").clicked() {
                        self.ask_ffmpeg = false;
                    }
                });
            });
        }
        if let Some((first, remaining)) = self.estimate {
            ui.label(format!(
                "Estimate: {} for the first keyframe; at most {} for the rest{}.",
                duration(first),
                duration(remaining),
                if self.remote { " of remote GPU time" } else { "" }
            ));
        }
        if let Some(reason) = &self.paused {
            ui.colored_label(
                Color32::from_rgb(230, 170, 90),
                format!("Paused: {reason}. Keyframes so far are saved; resume here or on the other engine."),
            );
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let Some(project) = &self.project else {
            ui.label("The preview appears once keyframes are rendered.");
            return;
        };
        if project.keyframes_done() == 0 {
            ui.label("The preview appears once keyframes are rendered.");
            return;
        }
        let dir = project.dir.clone();
        ui.horizontal(|ui| {
            ui.label("Preview");
            let total = self.preview_total.max(0.01);
            ui.add(Slider::new(&mut self.preview_t, 0.0..=total).suffix(" s").max_decimals(2));
        });
        let want = (self.generation, self.preview_t);
        if self.preview_asked != Some(want) {
            self.preview_asked = Some(want);
            let _ = self.preview_tx.send(PreviewReq { dir, generation: self.generation, t: self.preview_t });
        }
        if let Some(e) = &self.preview_error {
            ui.colored_label(Color32::from_rgb(230, 110, 90), e);
        }
        if let Some(tex) = &self.preview {
            let size = tex.size_vec2();
            let width = ui.available_width().min(size.x * 1.5);
            ui.image((tex.id(), size * (width / size.x)));
        }
    }
}

fn key_from(c: &Colouring, time: f64) -> ColourKey {
    ColourKey {
        time,
        offset: Some(c.offset),
        iter_div: Some(c.iter_div),
        slope_power: c.slopes.map(|s| s.power),
        slope_ratio: c.slopes.map(|s| s.ratio),
        slope_angle: c.slopes.map(|s| s.angle_deg),
    }
}

fn duration(s: f64) -> String {
    let s = s.max(0.0);
    if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{}m {:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{}h {:02}m", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    }
}

fn time_left(job: &Job) -> Option<String> {
    let (done, total) = job.progress;
    let made = done.checked_sub(job.progress_at_start).filter(|&n| n > 0)?;
    let rate = made as f64 / job.started.elapsed().as_secs_f64().max(1e-6);
    Some(duration(total.saturating_sub(done) as f64 / rate))
}

fn open_in_file_manager(dir: &Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

struct Notifier {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Notifier {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

/// Makes preview frames, the newest request first. Keeps the project and
/// its decoded keyframes between requests of the same generation.
fn preview_thread(rx: Receiver<PreviewReq>, tx: Sender<Msg>, ctx: egui::Context) {
    let send = |msg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    };
    let latest = |first: PreviewReq| {
        let mut req = first;
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        req
    };
    let mut pending = match rx.recv() {
        Ok(r) => Some(r),
        Err(_) => return,
    };
    while let Some(first) = pending.take() {
        let req = latest(first);
        let (dir, generation) = (req.dir.clone(), req.generation);
        let project = match Project::open(&dir) {
            Ok(p) => p,
            Err(e) => {
                send(Msg::Preview { generation, t: req.t, total: 0.0, image: Err(e) });
                pending = rx.recv().ok();
                continue;
            }
        };
        let ff = ffmpeg::find();
        let timeline = fracst_video::timeline(&project, ff.as_deref());
        let mut compositor = fracst_video::Compositor::new(&project);
        let spec = &project.spec;
        let k = PREVIEW_SIZE as f64 / spec.width.max(spec.height) as f64;
        let (w, h) = (((spec.width as f64 * k) as u32).max(16), ((spec.height as f64 * k) as u32).max(16));
        let mut req = Some(req);
        // Serve requests for this project until one comes for another.
        while let Some(r) = req.take() {
            if r.generation != generation || r.dir != dir {
                pending = Some(r);
                break;
            }
            let image = match &timeline {
                Ok(tl) => {
                    let t = r.t.clamp(0.0, tl.total);
                    compositor
                        .frame(tl, t, w, h, 1)
                        .map(|rgba| egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
                }
                Err(e) => Err(e.clone()),
            };
            let total = timeline.as_ref().map_or(0.0, |tl| tl.total);
            send(Msg::Preview { generation, t: r.t, total, image });
            match rx.recv() {
                Ok(next) => req = Some(latest(next)),
                Err(_) => return,
            }
        }
    }
}
