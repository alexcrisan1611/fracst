use crate::glide::{self, Glide};
use crate::prefs::{self, Prefs};
use crate::remote_config;
use crate::theme;
use crate::update::{self, Release};
use crate::video::VideoPanel;
use crate::window;
use crate::worker::{Outcome, RemoteConfig, RemoteStatus, Task, Worker};
use eframe::egui::{
    self, Align2, CollapsingHeader, Color32, ComboBox, DragValue, Id, Key, Modifiers, PointerButton, Pos2, Rect,
    RichText, Sense, Slider, StrokeKind, TextEdit, TextureHandle, TextureId, TextureOptions, Vec2, epaint::RectShape,
};
use fracst_core::{Colouring, FloatExp, Frame, Palette, Progress, Settings, Slopes, Transfer, View, kfr};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Disconnect from the remote GPU after this long without a render, so it
/// stops billing; the next render reconnects (in about a second while the
/// GPU is still up).
const REMOTE_IDLE: Duration = Duration::from_secs(60);

/// Width of the floating settings card.
const PANEL_WIDTH: f32 = 340.0;
/// Space between the canvas card's edge and what floats over it.
const INSET: f32 = 12.0;

/// The frame shown before a jump, fading out once the new one arrives.
struct Fade {
    texture: TextureHandle,
    rect: Rect,
    start: Option<Instant>,
}

struct Shown {
    view: View,
    frame: Frame,
    preview: bool,
    /// Set for frames rendered on a remote server.
    round_trip: Option<Duration>,
}

/// Editable text versions of the location, regenerated when the view changes.
#[derive(Default)]
struct Fields {
    view: Option<View>,
    re: String,
    im: String,
    zoom: String,
    iterations: String,
}

pub struct App {
    worker: Worker,
    view: View,
    settings: Settings,
    colouring: Colouring,
    shown: Option<Shown>,
    texture: Option<TextureHandle>,
    /// Where the texture was drawn last frame.
    placed: Option<Rect>,
    /// The view the display's glide is relative to.
    glide_target: Option<View>,
    glide: Option<Glide>,
    /// The next change to the view glides there, or cross-fades.
    animate: bool,
    jump: bool,
    fade: Option<Fade>,
    /// Testing hook: hold glides at this point.
    glide_freeze: Option<f64>,
    recolour: bool,
    /// Parameters of the last render submitted.
    requested: Option<(View, Settings)>,
    render_id: u64,
    /// A non-render task (minibrot search, export) that holds off rendering.
    blocking: Option<(u64, &'static str)>,
    last_render_secs: f64,
    resolution_scale: f32,
    /// Render only a centred region of this many pixels, not the whole canvas.
    region: Option<[u32; 2]>,
    /// The last region size chosen, kept while the region is off.
    region_size: [u32; 2],
    /// The region changed: rescale the zoom so pixels keep their size.
    region_changed: bool,
    zoom_factor: f64,
    minibrot_fill: f64,
    palette_seed: u64,
    fields: Fields,
    status: String,
    show_location: bool,
    location_text: String,
    show_help: bool,
    export_size: [u32; 2],
    export_supersample: u32,
    remote_url: String,
    remote_key: String,
    remote_status: RemoteStatus,
    /// Of the latest connect/disconnect, to ignore stale reports.
    remote_generation: u64,
    /// When the remote GPU last had work.
    remote_used: Instant,
    /// Whether to look for a newer release at startup.
    update_check: bool,
    /// The answer to the startup check, while it is pending.
    update_rx: Option<Receiver<Release>>,
    /// A newer release, until dismissed.
    update: Option<Release>,
    video: VideoPanel,
    prefs: Prefs,
    logo: Option<TextureHandle>,
    /// The theme's accent, which follows the palette.
    accent: Color32,
    /// Testing hook: save a screenshot of the window here once rendering settles, then exit.
    screenshot_to: Option<std::path::PathBuf>,
    /// With the screenshot: first zoom in, held this far through the glide.
    screenshot_glide: Option<f64>,
    screenshot_frames: u32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        // The server: from the environment, else the last one connected to,
        // else the default this build was made with.
        let saved = remote_config::load();
        let nonempty = |s: String| (!s.is_empty()).then_some(s);
        let auto_connect = env("FRACST_REMOTE_URL").is_some();
        let remote_url = env("FRACST_REMOTE_URL")
            .or_else(|| nonempty(saved.url.clone()))
            .or_else(|| option_env!("FRACST_DEFAULT_REMOTE_URL").map(String::from).and_then(nonempty))
            .unwrap_or_default();
        let remote_key =
            env("FRACST_REMOTE_KEY").or_else(|| env("FAL_KEY")).or_else(|| nonempty(saved.key)).unwrap_or_default();
        let screenshot_to: Option<std::path::PathBuf> = std::env::var_os("FRACST_SCREENSHOT").map(Into::into);
        let update_check = update::enabled();
        let update_rx = (update_check && screenshot_to.is_none()).then(|| update::spawn_check(cc.egui_ctx.clone()));
        let colouring = Colouring::default();
        let accent = theme::accent_from(colouring.palette.keys());
        theme::install(&cc.egui_ctx, accent);
        window::round_corners(cc);
        let logo =
            fracst_core::image::decode_rgba_png(include_bytes!("../../../assets/icon.png")).ok().map(|(rgba, w, h)| {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                cc.egui_ctx.load_texture("logo", img, TextureOptions::LINEAR)
            });
        let mut app = App {
            worker: Worker::spawn(cc.egui_ctx.clone()),
            view: View::default(),
            settings: Settings { auto_iterations: true, ..Settings::default() },
            colouring,
            shown: None,
            texture: None,
            placed: None,
            glide_target: None,
            glide: None,
            animate: false,
            jump: false,
            fade: None,
            glide_freeze: None,
            recolour: false,
            requested: None,
            render_id: 0,
            blocking: None,
            last_render_secs: 0.0,
            resolution_scale: 1.0,
            region: None,
            region_size: [256, 256],
            region_changed: false,
            zoom_factor: 4.0,
            minibrot_fill: 0.25,
            palette_seed: 1,
            fields: Fields::default(),
            status: "Left click: zoom in · right click: zoom out · wheel: zoom at cursor · drag: pan".into(),
            show_location: false,
            location_text: String::new(),
            show_help: false,
            export_size: [1920, 1080],
            export_supersample: 2,
            remote_url,
            remote_key,
            remote_status: RemoteStatus::Off,
            remote_generation: 0,
            remote_used: Instant::now(),
            update_check,
            update_rx,
            update: None,
            video: VideoPanel::new(cc.egui_ctx.clone()),
            prefs: Prefs::load(),
            logo,
            accent,
            screenshot_to,
            screenshot_glide: env("FRACST_SCREENSHOT_GLIDE").and_then(|v| v.trim().parse().ok()),
            screenshot_frames: 0,
        };
        // Connecting may start a paid GPU, so only when asked to.
        if auto_connect {
            app.connect_remote();
        }
        app
    }

    fn connect_remote(&mut self) {
        let config = RemoteConfig { url: self.remote_url.trim().to_string(), key: self.remote_key.trim().to_string() };
        self.remote_generation = self.worker.connect_remote(config);
        self.remote_status = RemoteStatus::Connecting(String::new());
        self.remote_used = Instant::now();
    }

    /// Pause the remote connection once it has been idle for a while, and
    /// otherwise make sure the UI wakes up to check again.
    fn pause_idle_remote(&mut self, ctx: &egui::Context) {
        if !matches!(self.remote_status, RemoteStatus::Connected(_)) {
            return;
        }
        if self.worker.current().is_some() {
            self.remote_used = Instant::now();
        }
        let idle = self.remote_used.elapsed();
        if idle >= REMOTE_IDLE {
            self.remote_generation = self.worker.disconnect_remote();
            self.remote_status = RemoteStatus::Paused;
        } else {
            ctx.request_repaint_after(REMOTE_IDLE - idle);
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg.outcome {
                Outcome::Frame { view, frame, preview, round_trip } => {
                    let frame = *frame;
                    if msg.id != self.render_id {
                        continue;
                    }
                    if !preview {
                        self.worker.finished(msg.id);
                        self.last_render_secs = round_trip.unwrap_or(frame.stats.total).as_secs_f64();
                        // Start the next frame from the limit this one suggests. The
                        // frame on screen is already complete, so don't re-render it.
                        let suggested = frame.stats.suggested_maxiter;
                        let same_place = View { maxiter: self.view.maxiter, ..view.clone() } == self.view;
                        if self.settings.auto_iterations && same_place && suggested != self.view.maxiter {
                            self.view.maxiter = suggested;
                            if let Some((requested, _)) = &mut self.requested {
                                requested.maxiter = suggested;
                            }
                        }
                    }
                    self.shown = Some(Shown { view, frame, preview, round_trip });
                    self.recolour = true;
                }
                Outcome::Remote { generation, status } => {
                    if generation == self.remote_generation {
                        // Show the view rendered where it now renders, and
                        // remember the server for next time.
                        if matches!(status, RemoteStatus::Connected(_)) {
                            self.requested = None;
                            remote_config::save(&remote_config::Saved {
                                url: self.remote_url.trim().to_string(),
                                key: self.remote_key.trim().to_string(),
                            });
                        }
                        self.remote_status = status;
                    }
                }
                Outcome::Minibrot(result) => {
                    self.worker.finished(msg.id);
                    self.blocking = None;
                    match result {
                        Ok((view, period)) => {
                            self.status = format!("Found minibrot of period {period}.");
                            self.view = View { maxiter: self.view.maxiter.max(period * 8), ..view };
                            self.jump = true;
                        }
                        Err(e) => self.status = e,
                    }
                }
                Outcome::Exported(result) => {
                    self.worker.finished(msg.id);
                    self.blocking = None;
                    self.status = match result {
                        Ok((path, t)) => format!("Saved {} in {:.2}s.", path.display(), t.as_secs_f64()),
                        Err(e) => format!("Export failed: {e}"),
                    };
                }
            }
        }
        if self.recolour {
            self.recolour = false;
            if let Some(s) = &self.shown {
                let rgba = self.colouring.apply(&s.frame);
                let img =
                    egui::ColorImage::from_rgba_unmultiplied([s.frame.width as usize, s.frame.height as usize], &rgba);
                match &mut self.texture {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => self.texture = Some(ctx.load_texture("fractal", img, TextureOptions::LINEAR)),
                }
            }
        }
    }

    fn request_render(&mut self) {
        if self.blocking.is_some() {
            return;
        }
        self.view.ensure_precision(self.settings.height);
        let want = (self.view.clone(), self.settings.clone());
        if self.requested.as_ref() != Some(&want) {
            if matches!(self.remote_status, RemoteStatus::Paused) {
                // Renders here until the GPU answers again.
                self.connect_remote();
            }
            self.remote_used = Instant::now();
            let preview = self.last_render_secs > 0.15;
            self.render_id =
                self.worker.submit(Task::Render { view: want.0.clone(), settings: want.1.clone(), preview });
            self.requested = Some(want);
        }
    }

    fn start_blocking(&mut self, task: Task, label: &'static str) {
        let id = self.worker.submit(task);
        self.blocking = Some((id, label));
        // Whatever render was in flight got cancelled; redo it afterwards.
        self.requested = None;
    }

    // ---- canvas -------------------------------------------------------------

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let px_per_point = ui.ctx().pixels_per_point() * self.resolution_scale;
        let rect = match self.region {
            Some([w, h]) => {
                Rect::from_center_size(full.center(), Vec2::new(w as f32, h as f32) / px_per_point).intersect(full)
            }
            None => full,
        };
        let old_height = self.settings.height;
        self.settings.width = ((rect.width() * px_per_point).round() as u32).max(16);
        self.settings.height = ((rect.height() * px_per_point).round() as u32).max(16);
        let (w, h) = (self.settings.width, self.settings.height);
        if std::mem::take(&mut self.region_changed) && old_height != h {
            // The zoom is relative to the image height: keep the pixels' size,
            // so the region shows the middle of what was on screen.
            self.view.zoom = self.view.zoom * FloatExp::from(h as f64 / old_height as f64);
        }
        // Changes made elsewhere since the last frame.
        self.sync_display();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());

        // Pointer positions in the target view's pixels: what is under the
        // pointer is what is displayed there, even mid-glide.
        let offset = self.offset();
        let to_pixels = |p: Pos2| {
            let (x, y) = canvas_pixels(p, rect, px_per_point);
            let t = offset.to_target([x - w as f64 / 2.0, y - h as f64 / 2.0]);
            (t[0] + w as f64 / 2.0, t[1] + h as f64 / 2.0)
        };
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = to_pixels(p);
            self.zoom_in_at(x, y);
        }
        if response.secondary_clicked() {
            self.zoom_out();
        }
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta() * px_per_point * offset.k as f32;
            if d != Vec2::ZERO {
                self.pan(d.x as f64, d.y as f64);
            }
        }
        if let Some(p) = response.hover_pos() {
            // Gliding smooths the wheel itself; otherwise let egui smooth it.
            let gliding = !self.glide_length().is_zero();
            let scroll = ui.input(|i| if gliding { wheel_points(i) } else { i.smooth_scroll_delta.y });
            if scroll != 0.0 {
                let (x, y) = to_pixels(p);
                let factor = 2f64.powf(scroll as f64 / 100.0);
                if factor > 1.0 || self.view.zoom > FloatExp::from(0.25) {
                    self.view.zoom_at(x, y, w, h, factor);
                    // What is under the pointer ends up under it once the glide ends.
                    let (cx, cy) = canvas_pixels(p, rect, px_per_point);
                    self.pan(cx - x, cy - y);
                    self.animate = true;
                }
            }
        }
        self.sync_display();

        ui.painter().rect_filled(full, theme::RADIUS, Color32::BLACK);
        if self.region.is_some() {
            ui.painter().rect_stroke(rect, theme::RADIUS, (1.0, Color32::from_gray(90)), StrokeKind::Outside);
        }
        self.placed = None;
        if let (Some(tex), Some(shown)) = (&self.texture, &self.shown) {
            let target = self.placement(rect, &shown.view, px_per_point);
            paint_rounded(ui.painter(), rect, target, tex.id(), Color32::WHITE);
            self.placed = Some(target);
        }
        let length = self.glide_length().as_secs_f64().max(1e-3);
        if let Some(fade) = &mut self.fade {
            // The old frame stays until the new one arrives, then fades out.
            if fade.start.is_none() && self.texture.is_some() {
                fade.start = Some(Instant::now());
            }
            let progress = |start: Instant| self.glide_freeze.unwrap_or(start.elapsed().as_secs_f64() / length);
            let left = fade.start.map_or(1.0, |start| 1.0 - glide::ease(progress(start).min(1.0)));
            if left <= 0.0 {
                self.fade = None;
            } else {
                let tint = Color32::WHITE.linear_multiply(left as f32);
                paint_rounded(ui.painter(), rect, fade.rect, fade.texture.id(), tint);
            }
        }
        let gliding = self.glide.is_some_and(|g| g.progress(Instant::now(), self.glide_length()) < 1.0);
        if gliding || self.fade.as_ref().is_some_and(|f| f.start.is_some()) {
            ui.ctx().request_repaint();
        }
    }

    /// Move the view so its contents shift by this many pixels.
    fn pan(&mut self, dx: f64, dy: f64) {
        let (w, h) = (self.settings.width, self.settings.height);
        let (dx, dy) = self.view.pixel_offset(w as f64 / 2.0 - dx, h as f64 / 2.0 - dy, w, h);
        self.view.translate_pixels(dx, dy, h);
    }

    /// Centre on a point (in pixels from the top left) and zoom in.
    fn zoom_in_at(&mut self, x: f64, y: f64) {
        let (w, h) = (self.settings.width, self.settings.height);
        let (dx, dy) = self.view.pixel_offset(x, y, w, h);
        self.view.translate_pixels(dx, dy, h);
        self.view.zoom = self.view.zoom * FloatExp::from(self.zoom_factor);
        self.animate = true;
    }

    fn zoom_out(&mut self) {
        self.view.zoom = (self.view.zoom / FloatExp::from(self.zoom_factor)).max(FloatExp::from(0.25));
        self.animate = true;
    }

    /// How long zooms glide and jumps fade for; zero when animation is off.
    fn glide_length(&self) -> Duration {
        self.prefs.glide()
    }

    /// How far the display is from the target view right now.
    fn offset(&self) -> glide::Offset {
        let Some(g) = &self.glide else { return glide::Offset::NONE };
        let progress = self.glide_freeze.unwrap_or_else(|| g.progress(Instant::now(), self.glide_length()));
        g.at(progress)
    }

    /// Follow changes to the view: glide to zooms, cross-fade to jumps, and
    /// move with everything else.
    fn sync_display(&mut self) {
        let Some(old) = self.glide_target.as_ref() else {
            self.glide_target = Some(self.view.clone());
            return;
        };
        if *old == self.view {
            self.animate = false;
            self.jump = false;
            return;
        }
        let gliding = !self.glide_length().is_zero();
        if std::mem::take(&mut self.jump) {
            self.glide = None;
            self.animate = false;
            if gliding && let (Some(texture), Some(rect)) = (self.texture.take(), self.placed) {
                // Hold the old frame, not a recolouring of it, until the new one is ready.
                self.shown = None;
                self.fade = Some(Fade { texture, rect, start: None });
            }
        } else if std::mem::take(&mut self.animate) && gliding {
            let ratio = (self.view.zoom / old.zoom).to_f64();
            let from = self.offset().retarget(ratio, centre_offset(old, &self.view, self.settings.height));
            let sane = (1e-6..1e6).contains(&from.k) && from.d.iter().all(|v| v.is_finite());
            self.glide = sane.then(|| Glide::new(from, Instant::now()));
        }
        self.glide_target = Some(self.view.clone());
    }

    /// Where the displayed texture (rendered for `shown`) lands for what is
    /// displayed now, so zooming and panning respond instantly while the new
    /// render runs.
    fn placement(&self, rect: Rect, shown: &View, px_per_point: f32) -> Rect {
        let scale = (self.view.zoom / shown.zoom).to_f64().clamp(1e-6, 1e6);
        let (c, s) = self.offset().to_display(centre_offset(shown, &self.view, self.settings.height));
        let clamp = |v: f64| v.clamp(-1e7, 1e7) as f32;
        let centre = rect.center() + Vec2::new(clamp(c[0]), clamp(c[1])) / px_per_point;
        Rect::from_center_size(centre, rect.size() * (scale * s).clamp(1e-6, 1e6) as f32)
    }

    // ---- side panel -----------------------------------------------------------

    fn controls(&mut self, ui: &mut egui::Ui) {
        fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
            CollapsingHeader::new(RichText::new(title).font(theme::strong(14.0)).color(theme::TEXT))
                .default_open(open)
                .show(ui, |ui| {
                    add(ui);
                    ui.add_space(4.0);
                });
        }
        section(ui, "Location", true, |ui| self.location_ui(ui));
        section(ui, "Navigation", true, |ui| self.navigation_ui(ui));
        section(ui, "Colour", true, |ui| self.colour_ui(ui));
        section(ui, "Rendering", false, |ui| self.rendering_ui(ui));
        section(ui, "Remote GPU", !self.remote_url.is_empty(), |ui| self.remote_ui(ui));
        section(ui, "Export", false, |ui| self.export_ui(ui));
        section(ui, "Statistics", true, |ui| self.stats_ui(ui));
        section(ui, "App", false, |ui| self.app_ui(ui));
    }

    fn app_ui(&mut self, ui: &mut egui::Ui) {
        if ui.checkbox(&mut self.update_check, "Check for updates at startup").changed() {
            update::set_enabled(self.update_check);
        }
        if ui.button("Controls…").clicked() {
            self.show_help = true;
        }
    }

    fn sync_fields(&mut self) {
        if self.fields.view.as_ref() != Some(&self.view) {
            self.fields = Fields {
                view: Some(self.view.clone()),
                re: self.view.re_string(),
                im: self.view.im_string(),
                zoom: format!("{:.6}", self.view.zoom),
                iterations: self.view.maxiter.to_string(),
            };
        }
    }

    fn location_ui(&mut self, ui: &mut egui::Ui) {
        self.sync_fields();
        fn mono(s: &mut String) -> TextEdit<'_> {
            TextEdit::multiline(s).desired_rows(1).code_editor().desired_width(f32::INFINITY)
        }
        ui.label("Real");
        ui.add(mono(&mut self.fields.re));
        ui.label("Imaginary");
        ui.add(mono(&mut self.fields.im));
        egui::Grid::new("zoom-iter").num_columns(2).show(ui, |ui| {
            ui.label("Zoom");
            ui.add(TextEdit::singleline(&mut self.fields.zoom).desired_width(140.0));
            ui.end_row();
            ui.label("Iterations");
            ui.add(TextEdit::singleline(&mut self.fields.iterations).desired_width(140.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("Go").clicked() {
                let maxiter = self.fields.iterations.trim().parse().unwrap_or(self.view.maxiter);
                match View::parse(&self.fields.re, &self.fields.im, &self.fields.zoom, maxiter) {
                    Some(v) => {
                        self.view = View { imag_down: self.view.imag_down, ..v };
                        self.jump = true;
                    }
                    None => self.status = "Could not parse the location.".into(),
                }
            }
            if ui.button("Reset").clicked() {
                self.view = View::default();
                self.jump = true;
            }
            if ui.button("Copy").on_hover_text("Copy the location as .kfr text").clicked() {
                ui.ctx().copy_text(kfr::to_string(&self.view, &self.colouring));
                self.status = "Location copied to the clipboard.".into();
            }
            if ui.button("Paste…").on_hover_text("Paste a location in .kfr text format").clicked() {
                self.location_text.clear();
                self.show_location = true;
            }
        });
    }

    fn navigation_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Click zoom");
            ComboBox::from_id_salt("zoom-factor").selected_text(format!("×{}", self.zoom_factor)).show_ui(ui, |ui| {
                for f in [2.0, 4.0, 8.0, 16.0, 32.0] {
                    ui.selectable_value(&mut self.zoom_factor, f, format!("×{f}"));
                }
            });
            if ui.button("Zoom out").clicked() {
                self.zoom_out();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Zoom animation");
            let current = prefs::glide_name(self.prefs.glide_ms);
            ComboBox::from_id_salt("glide").selected_text(current).show_ui(ui, |ui| {
                for (name, ms) in prefs::GLIDES {
                    if ui.selectable_value(&mut self.prefs.glide_ms, ms, name).changed() {
                        self.prefs.save();
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            let busy = self.blocking.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new("Find minibrot"))
                .on_hover_text("Newton's method: jump to the lowest-period minibrot near the centre")
                .clicked()
            {
                let task = Task::FindMinibrot {
                    view: self.view.clone(),
                    height: self.settings.height,
                    fill: self.minibrot_fill,
                };
                self.start_blocking(task, "Finding minibrot");
            }
            ui.add(Slider::new(&mut self.minibrot_fill, 0.001..=2.0).logarithmic(true).text("fill"))
                .on_hover_text("How large the minibrot appears (1 = full height)");
        });
        ui.checkbox(&mut self.settings.auto_iterations, "Auto iterations").on_hover_text(
            "Choose the iteration limit per frame: enough for any minibrot near the view,              raised when pixels are cut off, lowered again when the view needs less",
        );
    }

    fn colour_ui(&mut self, ui: &mut egui::Ui) {
        let before = self.colouring.clone();
        let c = &mut self.colouring;
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("palette").selected_text("Palette preset").show_ui(ui, |ui| {
                for (name, p) in Palette::presets() {
                    if ui.selectable_label(c.palette == p, name).clicked() {
                        c.palette = p;
                    }
                }
            });
            if ui.button("Random").clicked() {
                self.palette_seed += 1;
                let n = 4 + (self.palette_seed % 6) as usize;
                c.palette = Palette::random(self.palette_seed, n);
            }
        });
        let mut keys = c.palette.keys().to_vec();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for k in keys.iter_mut() {
                changed |= ui.color_edit_button_srgb(k).changed();
            }
            if ui.small_button("+").clicked() {
                keys.push(*keys.last().unwrap_or(&[128, 128, 128]));
                changed = true;
            }
            if keys.len() > 2 && ui.small_button("−").clicked() {
                keys.pop();
                changed = true;
            }
        });
        if changed {
            c.palette = Palette::new(keys);
        }
        ui.add(Slider::new(&mut c.iter_div, 0.0001..=10000.0).logarithmic(true).text("iter div"))
            .on_hover_text("Iterations per palette step (IterDiv). Smaller = more colour bands");
        ui.add(Slider::new(&mut c.offset, 0.0..=1024.0).text("offset"));
        ComboBox::from_label("transfer").selected_text(c.transfer.name()).show_ui(ui, |ui| {
            for t in Transfer::ALL {
                ui.selectable_value(&mut c.transfer, t, t.name());
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.smooth, "Smooth");
            ui.checkbox(&mut c.flat, "Flat");
            ui.label("Interior");
            ui.color_edit_button_srgb(&mut c.interior);
        });
        let mut slopes_on = c.slopes.is_some();
        ui.checkbox(&mut slopes_on, "Slope shading");
        match (slopes_on, &mut c.slopes) {
            (true, Some(s)) => {
                ui.add(Slider::new(&mut s.power, 1.0..=1000.0).logarithmic(true).text("power"));
                ui.add(Slider::new(&mut s.ratio, 0.0..=100.0).text("ratio %"));
                ui.add(Slider::new(&mut s.angle_deg, -180.0..=180.0).text("angle"));
            }
            (true, None) => c.slopes = Some(Slopes::default()),
            (false, _) => c.slopes = None,
        }
        if self.colouring != before {
            self.recolour = true;
        }
    }

    fn rendering_ui(&mut self, ui: &mut egui::Ui) {
        ComboBox::from_label("resolution").selected_text(format!("{:.0}%", self.resolution_scale * 100.0)).show_ui(
            ui,
            |ui| {
                for s in [0.25f32, 0.5, 1.0, 2.0] {
                    ui.selectable_value(&mut self.resolution_scale, s, format!("{:.0}%", s * 100.0));
                }
            },
        );
        let mut region = self.region.is_some();
        let mut changed = ui
            .checkbox(&mut region, "Render only a region")
            .on_hover_text("Render a small centred part of the view, for slow deep locations; pixels keep their size")
            .changed();
        ui.add_enabled_ui(region, |ui| {
            ui.horizontal(|ui| {
                let [w, h] = &mut self.region_size;
                changed |= ui.add(DragValue::new(w).range(16..=16384)).changed();
                ui.label("×");
                changed |= ui.add(DragValue::new(h).range(16..=16384)).changed();
                ui.label("pixels");
            });
        });
        if changed {
            self.region = region.then_some(self.region_size);
            self.region_changed = true;
        }
        ui.checkbox(&mut self.settings.bla, "Bilinear approximation (BLA)");
        let mut bits = (-self.settings.bla_epsilon.log2()).round() as u32;
        if ui
            .add(Slider::new(&mut bits, 16..=53).text("BLA precision bits"))
            .on_hover_text("Tolerance 2^-bits per skipped step; lower is faster but riskier")
            .changed()
        {
            self.settings.bla_epsilon = 0.5f64.powi(bits as i32);
        }
        ui.add(Slider::new(&mut self.settings.bailout, 2.0..=1e6).logarithmic(true).text("bailout"));
        ui.checkbox(&mut self.settings.simd, "SIMD (AVX2)")
            .on_hover_text("Iterate four pixels at once where it pays; the image is identical either way");
        ui.checkbox(&mut self.settings.interior_detection, "Interior detection")
            .on_hover_text("Stop iterating pixels that are attracted to a cycle (big win inside minibrots)");
        ui.checkbox(&mut self.settings.periodic_reference, "Snap reference to nearby minibrot").on_hover_text(
            "Use the nucleus of a minibrot near the view as the reference: its orbit stops after one period",
        );
        ui.checkbox(&mut self.settings.reuse_reference, "Reuse reference orbit while nearby");
        ui.checkbox(&mut self.view.imag_down, "Imaginary axis points down")
            .on_hover_text("The default for .kfr files without ImagPointsUp");
    }

    fn remote_ui(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("remote").num_columns(2).show(ui, |ui| {
            ui.label("Server");
            ui.add(
                TextEdit::singleline(&mut self.remote_url)
                    .hint_text("wss://fracst-gateway.you.workers.dev/stream")
                    .desired_width(230.0),
            )
            .on_hover_text(
                "A fracst gateway's /stream address, a Modal or fal.ai deployment's address, or ws://host:7878 for a fracst-server",
            );
            ui.end_row();
            ui.label("Key");
            ui.add(
                TextEdit::singleline(&mut self.remote_key)
                    .password(true)
                    .hint_text("fracst key (fst_…), Modal or fal token")
                    .desired_width(230.0),
            );
            ui.end_row();
        });
        let active = matches!(
            self.remote_status,
            RemoteStatus::Connecting(_) | RemoteStatus::Connected(_) | RemoteStatus::Paused
        );
        ui.horizontal(|ui| {
            let label = if active { "Reconnect" } else { "Connect" };
            if ui.add_enabled(!self.remote_url.trim().is_empty(), egui::Button::new(label)).clicked() {
                self.connect_remote();
            }
            if active && ui.button("Disconnect").clicked() {
                self.remote_generation = self.worker.disconnect_remote();
                self.remote_status = RemoteStatus::Off;
                self.requested = None;
            }
        });
        match &self.remote_status {
            RemoteStatus::Off => {
                ui.label("Rendering on this computer.");
            }
            RemoteStatus::Connecting(message) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    if message.is_empty() {
                        ui.label("Connecting… a GPU that was idle can take a minute or two to start.");
                    } else {
                        ui.label(message);
                    }
                });
            }
            RemoteStatus::Connected(engine) => {
                ui.label(format!("Rendering on {engine}"));
            }
            RemoteStatus::Paused => {
                ui.label("Paused while idle, so the GPU can shut down. The next render reconnects.");
            }
            RemoteStatus::Failed(e) => {
                ui.colored_label(Color32::from_rgb(230, 110, 90), format!("Remote: {e}"));
            }
        }
    }

    fn export_ui(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("export").num_columns(2).show(ui, |ui| {
            ui.label("Size");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut self.export_size[0]).range(16..=32768));
                ui.label("×");
                ui.add(DragValue::new(&mut self.export_size[1]).range(16..=32768));
            });
            ui.end_row();
            ui.label("Supersample");
            ui.add(DragValue::new(&mut self.export_supersample).range(1..=4).suffix("×"));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(self.blocking.is_none(), egui::Button::new("Export PNG…")).clicked() {
                self.export_dialog();
            }
            if ui.button("Make video…").on_hover_text("A zoom video from the whole set into this location").clicked()
            {
                self.make_video();
            }
        });
    }

    fn export_dialog(&mut self) {
        if let Some(path) =
            rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("fracst.png").save_file()
        {
            let settings =
                Settings { width: self.export_size[0], height: self.export_size[1], ..self.settings.clone() };
            let task = Task::Export {
                view: self.view.clone(),
                settings,
                colouring: self.colouring.clone(),
                supersample: self.export_supersample,
                path,
            };
            self.start_blocking(task, "Exporting");
        }
    }

    fn stats_ui(&mut self, ui: &mut egui::Ui) {
        let Some(shown) = &self.shown else {
            ui.label("Nothing rendered yet.");
            return;
        };
        let f = &shown.frame;
        let st = &f.stats;
        let px = (f.width as f64 * f.height as f64).max(1.0);
        egui::Grid::new("stats").num_columns(2).striped(true).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            };
            row("Frame", format!("{}×{}{}", f.width, f.height, if shown.preview { " (preview)" } else { "" }));
            if let Some(rt) = shown.round_trip {
                row("Round trip", format!("{:.3} s (server {:.3} s)", rt.as_secs_f64(), st.total.as_secs_f64()));
                if let RemoteStatus::Connected(engine) = &self.remote_status {
                    row("Rendered on", engine.split(" · ").next().unwrap_or(engine).to_string());
                }
            } else {
                row("Total", format!("{:.3} s", st.total.as_secs_f64()));
            }
            row(
                "Iteration limit",
                if st.rounds > 1 { format!("{} ({} passes)", f.maxiter, st.rounds) } else { f.maxiter.to_string() },
            );
            if let Some(p) = st.nearby_period {
                row("Nearby minibrot", format!("period {p}"));
            }
            row(
                "Reference",
                if st.reference_reused {
                    format!("reused, {} iters", st.reference_len)
                } else {
                    format!(
                        "{:.3} s, {} iters{}",
                        st.reference.as_secs_f64(),
                        st.reference_len,
                        if st.reference_on_nucleus {
                            " (on nucleus)"
                        } else if st.reference_truncated {
                            " (stopped early)"
                        } else {
                            ""
                        }
                    )
                },
            );
            row("Precision", format!("{} bits", 64 * (st.reference_limbs.max(1) - 1)));
            row("BLA table", format!("{:.3} s, {} levels", st.bla.as_secs_f64(), st.bla_levels));
            row("Pixels", format!("{:.3} s ({})", st.pixels.as_secs_f64(), st.mode));
            row("Avg iterations", format!("{:.0}", st.iterations as f64 / px));
            row(
                "Avg steps",
                format!("{:.1} ({:.0}× skipped)", st.steps as f64 / px, st.iterations as f64 / st.steps.max(1) as f64),
            );
            if st.max_escaped > 0 {
                row("Escaped", format!("{} … {}", st.min_escaped, st.max_escaped));
            }
            row("Interior", format!("{:.1}%", 100.0 * st.interior as f64 / px));
            if st.undecided > 0 {
                row("Reached limit", format!("{:.2}%", 100.0 * st.undecided as f64 / px));
            }
        });
    }

    // ---- menus, windows, status ------------------------------------------------

    /// The floating pill at the top left: the wordmark and the main actions.
    fn toolbar(&mut self, ctx: &egui::Context, canvas: Rect) {
        egui::Area::new(Id::unique("toolbar")).fixed_pos(canvas.min + Vec2::splat(INSET)).show(ctx, |ui| {
            theme::pill().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // The wordmark doubles as a handle for moving the window.
                    let mark = ui
                        .scope(|ui| {
                            if let Some(logo) = &self.logo {
                                ui.add(egui::Image::new((logo.id(), Vec2::splat(18.0))));
                            }
                            ui.label(RichText::new("fracst").font(theme::strong(14.5)).color(theme::TEXT));
                        })
                        .response
                        .interact(Sense::click_and_drag());
                    window::drag_window(ctx, &mark);
                    ui.add_space(6.0);
                    let tool = |ui: &mut egui::Ui, text: &str, hint: &str| {
                        ui.add(egui::Button::new(text).frame_when_inactive(false)).on_hover_text(hint).clicked()
                    };
                    if tool(ui, "Open", "Open a location or video project  (Ctrl+O)") {
                        self.open_dialog();
                    }
                    if tool(ui, "Save", "Save the location  (Ctrl+S)") {
                        self.save_dialog();
                    }
                    if tool(ui, "Export", "Export a PNG  (Ctrl+E)") && self.blocking.is_none() {
                        self.export_dialog();
                    }
                    let video = egui::Button::new("Video").frame_when_inactive(false);
                    egui::containers::menu::MenuButton::from_button(video).ui(ui, |ui| {
                        if ui.button("Make video…").clicked() {
                            self.make_video();
                        }
                        if ui.button("Open video project…").clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_title("Video project folder").pick_folder()
                        {
                            self.open_video_project(&dir);
                        }
                    });
                    let panel = if self.prefs.panel { "Hide settings  (Tab)" } else { "Show settings  (Tab)" };
                    if ui
                        .add(egui::Button::new("⚙").frame_when_inactive(false).selected(self.prefs.panel))
                        .on_hover_text(panel)
                        .clicked()
                    {
                        self.toggle_panel();
                    }
                    if tool(ui, "?", "Controls") {
                        self.show_help = true;
                    }
                    if let Some(release) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                        self.update = Some(release);
                    }
                    if let Some(release) = &self.update {
                        ui.separator();
                        ui.hyperlink_to(format!("{} is out", release.version), &release.url)
                            .on_hover_text("Open the release page to download it");
                        if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                            self.update = None;
                            self.update_rx = None;
                        }
                    }
                });
            });
        });
    }

    fn toggle_panel(&mut self) {
        self.prefs.panel = !self.prefs.panel;
        self.prefs.save();
    }

    /// The floating settings card on the right, which slides away with Tab.
    fn side_panel(&mut self, ctx: &egui::Context, canvas: Rect, top: f32) {
        let shown = ctx.animate_bool_with_time_and_easing(
            Id::unique("panel-shown"),
            self.prefs.panel,
            0.2,
            egui::emath::easing::cubic_out,
        );
        if shown == 0.0 {
            return;
        }
        let slide = (1.0 - shown) * (PANEL_WIDTH + 2.0 * INSET);
        let pos = Pos2::new(canvas.right() - INSET - PANEL_WIDTH + slide, top);
        let height = (canvas.bottom() - INSET - top).max(120.0);
        egui::Area::new(Id::unique("settings")).fixed_pos(pos).show(ctx, |ui| {
            ui.multiply_opacity(shown);
            theme::card().inner_margin(egui::Margin { left: 12, right: 6, top: 8, bottom: 8 }).show(ui, |ui| {
                let inner = Vec2::new(PANEL_WIDTH - 18.0, height - 16.0);
                ui.set_min_size(inner);
                ui.set_max_width(inner.x);
                egui::ScrollArea::vertical().max_height(inner.y).auto_shrink(false).show(ui, |ui| {
                    ui.add_space(2.0);
                    ui.set_max_width(inner.x - 10.0);
                    self.controls(ui);
                });
            });
        });
    }

    /// The floating pill at the bottom left: where we are, and what is running.
    fn status_pill(&mut self, ctx: &egui::Context, canvas: Rect) {
        let panel = if self.prefs.panel { PANEL_WIDTH + INSET } else { 0.0 };
        let max_width = (canvas.width() - 2.0 * INSET - panel - 24.0).max(200.0);
        egui::Area::new(Id::unique("status"))
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(Pos2::new(canvas.left() + INSET, canvas.bottom() - INSET))
            .show(ctx, |ui| {
                theme::pill().show(ui, |ui| {
                    ui.set_max_width(max_width);
                    ui.horizontal(|ui| {
                        let mono = |s: String| RichText::new(s).monospace().color(theme::TEXT);
                        ui.label(RichText::new("Zoom").color(theme::TEXT_WEAK));
                        ui.label(mono(format!("{:.4}", self.view.zoom)));
                        ui.add_space(4.0);
                        ui.label(RichText::new("Iterations").color(theme::TEXT_WEAK));
                        ui.label(mono(self.view.maxiter.to_string()));
                        ui.separator();
                        if let Some((_, p)) = self.worker.current() {
                            let (stage, frac) = p.snapshot();
                            let label = match (self.blocking, stage) {
                                (Some((_, l)), _) => l,
                                (None, Progress::REFERENCE) => "Reference orbit",
                                (None, Progress::BLA) => "BLA table",
                                (None, Progress::PIXELS) => "Pixels",
                                _ => "Starting",
                            };
                            ui.add(egui::ProgressBar::new(frac as f32).desired_width(170.0).text(label));
                            if ui.small_button("Cancel").on_hover_text("Esc").clicked()
                                || ui.input(|i| i.key_pressed(Key::Escape))
                            {
                                self.worker.cancel();
                                self.blocking = None;
                                // Do not immediately resubmit the cancelled render.
                                self.requested = Some((self.view.clone(), self.settings.clone()));
                            }
                            ui.ctx().request_repaint_after(Duration::from_millis(100));
                        } else {
                            ui.add(egui::Label::new(RichText::new(&self.status).color(theme::TEXT_WEAK)).truncate());
                        }
                    });
                });
            });
    }

    fn location_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_location;
        egui::Window::new("Location text").open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.label("Paste location text in .kfr format (Re:, Im:, Zoom:, Iterations:, Colors: …)");
            ui.add(
                TextEdit::multiline(&mut self.location_text)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    match kfr::parse(&self.location_text) {
                        Ok(loc) => {
                            self.view = loc.view;
                            self.colouring = loc.colouring;
                            self.recolour = true;
                            self.jump = true;
                            self.show_location = false;
                        }
                        Err(e) => self.status = format!("Could not read location: {e}"),
                    }
                }
                if ui.button("Current").clicked() {
                    self.location_text = kfr::to_string(&self.view, &self.colouring);
                }
            });
        });
        self.show_location &= open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Controls").open(&mut self.show_help).show(ctx, |ui| {
            ui.label("Left click — centre on the point and zoom in");
            ui.label("Right click — zoom out");
            ui.label("Mouse wheel — zoom around the cursor");
            ui.label("Drag — pan");
            ui.label("Esc — cancel the current render");
            ui.label("Tab — show or hide the settings");
            ui.label("Ctrl+O / Ctrl+S / Ctrl+E — open, save location, export PNG");
        });
    }

    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Location", &["kfr", "png"]).pick_file() else { return };
        self.open_path(&path);
    }

    fn make_video(&mut self) {
        let location = kfr::Location { view: self.view.clone(), colouring: self.colouring.clone(), size: None };
        self.video.start_new(location);
    }

    fn open_video_project(&mut self, dir: &Path) {
        match self.video.open_project(dir) {
            Ok(()) => {
                // Show the video's destination, so its colours can be tuned here.
                if let Some(loc) = self.video.project_location() {
                    self.view = loc.view.clone();
                    self.colouring = loc.colouring.clone();
                    self.recolour = true;
                    self.jump = true;
                }
                self.status = format!("Opened video project {}.", dir.display());
            }
            Err(e) => self.status = format!("Could not open the video project: {e}"),
        }
    }

    /// Open a location file, or a video project's folder.
    pub fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            self.open_video_project(path);
            return;
        }
        match kfr::load(path) {
            Ok(loc) => {
                self.view = loc.view;
                self.colouring = loc.colouring;
                self.recolour = true;
                self.jump = true;
                self.status = format!("Opened {}.", path.display());
            }
            Err(e) => self.status = format!("Could not open {}: {e}", path.display()),
        }
    }

    fn save_dialog(&mut self) {
        if let Some(path) =
            rfd::FileDialog::new().add_filter("Location", &["kfr"]).set_file_name("location.kfr").save_file()
        {
            self.status = match kfr::save(&path, &self.view, &self.colouring) {
                Ok(()) => format!("Saved {}.", path.display()),
                Err(e) => format!("Could not save: {e}"),
            };
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (open, save, export) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (cmd && i.key_pressed(Key::O), cmd && i.key_pressed(Key::S), cmd && i.key_pressed(Key::E))
        });
        if open {
            self.open_dialog();
        }
        if save {
            self.save_dialog();
        }
        if export && self.blocking.is_none() {
            self.export_dialog();
        }
        // Tab moves between text fields while one is focused; otherwise it
        // shows and hides the settings.
        if ctx.memory(|m| m.focused().is_none()) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
            self.toggle_panel();
        }
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(path) = dropped.first() {
            self.open_path(path);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Transparent where the window's rounded corners are painted.
        if window::PAINT_CORNERS { [0.0; 4] } else { theme::WINDOW_BG.to_normalized_gamma_f32() }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_worker(&ctx);
        self.shortcuts(&ctx);
        let accent = theme::accent_from(self.colouring.palette.keys());
        if accent != self.accent {
            self.accent = accent;
            theme::set_accent(&ctx, accent);
        }
        let window = ui.max_rect();
        ui.painter().rect_filled(window, window::window_radius(&ctx), theme::WINDOW_BG);
        let mut canvas = window.shrink(theme::GAP);
        canvas.min.y = window.top() + window::top_gap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| self.canvas(ui));
        window::frame_controls(ui, window, canvas);
        self.toolbar(&ctx, canvas);
        self.status_pill(&ctx, canvas);
        let panel_top = window::buttons(&ctx, canvas, INSET).map_or(canvas.top() + INSET, |bottom| bottom + 8.0);
        self.side_panel(&ctx, canvas, panel_top);
        if self.show_location {
            self.location_window(&ctx);
        }
        if self.show_help {
            self.help_window(&ctx);
        }
        let remote = Some((self.remote_url.trim().to_string(), self.remote_key.trim().to_string()))
            .filter(|(url, _)| !url.is_empty());
        self.video.ui(&ctx, remote, &self.colouring);
        self.request_render();
        self.pause_idle_remote(&ctx);
        if self.recolour {
            ctx.request_repaint();
        }
        let settled = self.worker.current().is_none()
            && self.shown.as_ref().is_some_and(|s| !s.preview)
            && !matches!(self.remote_status, RemoteStatus::Connecting(_));
        if settled
            && self.screenshot_to.is_some()
            && let Some(progress) = self.screenshot_glide.take()
        {
            // Zoom into a point up and to the left, and hold the glide there.
            self.glide_freeze = Some(progress);
            self.zoom_in_at(self.settings.width as f64 * 0.3, self.settings.height as f64 * 0.3);
            ctx.request_repaint();
        } else if settled && let Some(path) = self.screenshot_to.take() {
            ctx.request_screenshot(move |img| {
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = fracst_core::image::save_png(&path, &rgba, img.size[0] as u32, img.size[1] as u32, None);
                std::process::exit(0);
            });
            self.screenshot_frames = 60;
        }
        if self.screenshot_frames > 0 {
            // The capture happens on a later frame; keep frames coming until it does.
            self.screenshot_frames -= 1;
            ctx.request_repaint();
        }
    }
}

/// Paint the part of a texture placed at `target` that falls inside `clip`,
/// with rounded corners.
fn paint_rounded(painter: &egui::Painter, clip: Rect, target: Rect, texture: TextureId, tint: Color32) {
    let visible = clip.intersect(target);
    if !visible.is_positive() {
        return;
    }
    let to_uv = |p: Pos2| Pos2::new((p.x - target.min.x) / target.width(), (p.y - target.min.y) / target.height());
    let uv = Rect::from_min_max(to_uv(visible.min), to_uv(visible.max));
    painter.add(RectShape::filled(visible, theme::RADIUS, tint).with_texture(texture, uv));
}

/// A position on the canvas in its pixels, from the top left.
fn canvas_pixels(p: Pos2, rect: Rect, px_per_point: f32) -> (f64, f64) {
    (((p.x - rect.min.x) * px_per_point) as f64, ((p.y - rect.min.y) * px_per_point) as f64)
}

/// Where `of`'s centre is relative to `from`'s, in `from`'s pixels on screen.
fn centre_offset(of: &View, from: &View, height: u32) -> [f64; 2] {
    let spacing = from.pixel_spacing(height);
    let n = of.re.limbs().max(from.re.limbs());
    let d_re = of.re.with_limbs(n).sub(&from.re.with_limbs(n)).to_floatexp() / spacing;
    let d_im = of.im.with_limbs(n).sub(&from.im.with_limbs(n)).to_floatexp() / spacing;
    let clamp = |v: FloatExp| v.to_f64().clamp(-1e12, 1e12);
    [clamp(d_re), if from.imag_down { clamp(d_im) } else { -clamp(d_im) }]
}

/// This frame's mouse wheel movement, unsmoothed, in points (as egui counts them).
fn wheel_points(input: &egui::InputState) -> f32 {
    let mut points = 0.0;
    for event in &input.raw.events {
        if let egui::Event::MouseWheel { unit, delta, .. } = event {
            points += match unit {
                egui::MouseWheelUnit::Point => delta.y,
                egui::MouseWheelUnit::Line => 40.0 * delta.y,
                egui::MouseWheelUnit::Page => input.content_rect().height() * delta.y,
            };
        }
    }
    points
}
