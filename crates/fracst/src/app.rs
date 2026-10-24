use crate::worker::{Outcome, Task, Worker};
use eframe::egui::{
    self, CollapsingHeader, Color32, ComboBox, DragValue, Key, PointerButton, Pos2, Rect, Sense, Slider, TextEdit,
    TextureHandle, TextureOptions, Vec2, ViewportCommand,
};
use fracst_core::{Colouring, FloatExp, Frame, Palette, Progress, Settings, Slopes, Transfer, View, kfr};
use std::path::Path;
use std::time::Duration;

struct Shown {
    view: View,
    frame: Frame,
    preview: bool,
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
    recolour: bool,
    /// Parameters of the last render submitted.
    requested: Option<(View, Settings)>,
    render_id: u64,
    /// A non-render task (minibrot search, export) that holds off rendering.
    blocking: Option<(u64, &'static str)>,
    last_render_secs: f64,
    resolution_scale: f32,
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
    /// Testing hook: save a screenshot of the window here once rendering settles, then exit.
    screenshot_to: Option<std::path::PathBuf>,
    screenshot_frames: u32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        App {
            worker: Worker::spawn(cc.egui_ctx.clone()),
            view: View::default(),
            settings: Settings { auto_iterations: true, ..Settings::default() },
            colouring: Colouring::default(),
            shown: None,
            texture: None,
            recolour: false,
            requested: None,
            render_id: 0,
            blocking: None,
            last_render_secs: 0.0,
            resolution_scale: 1.0,
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
            screenshot_to: std::env::var_os("FRACST_SCREENSHOT").map(Into::into),
            screenshot_frames: 0,
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg.outcome {
                Outcome::Frame { view, frame, preview } => {
                    let frame = *frame;
                    if msg.id != self.render_id {
                        continue;
                    }
                    if !preview {
                        self.worker.finished(msg.id);
                        self.last_render_secs = frame.stats.total.as_secs_f64();
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
                    self.shown = Some(Shown { view, frame, preview });
                    self.recolour = true;
                }
                Outcome::Minibrot(result) => {
                    self.worker.finished(msg.id);
                    self.blocking = None;
                    match result {
                        Ok((view, period)) => {
                            self.status = format!("Found minibrot of period {period}.");
                            self.view = View { maxiter: self.view.maxiter.max(period * 8), ..view };
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
        let rect = ui.max_rect();
        let px_per_point = ui.ctx().pixels_per_point() * self.resolution_scale;
        self.settings.width = ((rect.width() * px_per_point).round() as u32).max(16);
        self.settings.height = ((rect.height() * px_per_point).round() as u32).max(16);
        let (w, h) = (self.settings.width, self.settings.height);
        let response = ui.allocate_rect(rect, Sense::click_and_drag());

        if let (Some(tex), Some(shown)) = (&self.texture, &self.shown) {
            let target = self.placement(rect, &shown.view, px_per_point);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().with_clip_rect(rect).image(tex.id(), target, uv, Color32::WHITE);
        }

        let to_pixels =
            |p: Pos2| (((p.x - rect.min.x) * px_per_point) as f64, ((p.y - rect.min.y) * px_per_point) as f64);
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = to_pixels(p);
            let (dx, dy) = self.view.pixel_offset(x, y, w, h);
            self.view.translate_pixels(dx, dy, h);
            self.view.zoom = self.view.zoom * FloatExp::from(self.zoom_factor);
        }
        if response.secondary_clicked() {
            self.view.zoom = (self.view.zoom / FloatExp::from(self.zoom_factor)).max(FloatExp::from(0.25));
        }
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta() * px_per_point;
            if d != Vec2::ZERO {
                let (dx, dy) = self.view.pixel_offset(w as f64 / 2.0 - d.x as f64, h as f64 / 2.0 - d.y as f64, w, h);
                self.view.translate_pixels(dx, dy, h);
            }
        }
        if let Some(p) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let (x, y) = to_pixels(p);
                let factor = 2f64.powf(scroll as f64 / 100.0);
                if factor > 1.0 || self.view.zoom > FloatExp::from(0.25) {
                    self.view.zoom_at(x, y, w, h, factor);
                }
            }
        }
    }

    /// Where the displayed texture (rendered for `shown`) lands for the current view,
    /// so zooming and panning respond instantly while the new render runs.
    fn placement(&self, rect: Rect, shown: &View, px_per_point: f32) -> Rect {
        let scale = (self.view.zoom / shown.zoom).to_f64().clamp(1e-6, 1e6) as f32;
        let spacing = self.view.pixel_spacing(self.settings.height);
        let n = shown.re.limbs().max(self.view.re.limbs());
        let d_re = shown.re.with_limbs(n).sub(&self.view.re.with_limbs(n)).to_floatexp() / spacing;
        let d_im = shown.im.with_limbs(n).sub(&self.view.im.with_limbs(n)).to_floatexp() / spacing;
        let clamp = |v: FloatExp| v.to_f64().clamp(-1e7, 1e7) as f32;
        let dy = if self.view.imag_down { clamp(d_im) } else { -clamp(d_im) };
        let centre = rect.center() + Vec2::new(clamp(d_re), dy) / px_per_point;
        Rect::from_center_size(centre, rect.size() * scale)
    }

    // ---- side panel -----------------------------------------------------------

    fn controls(&mut self, ui: &mut egui::Ui) {
        CollapsingHeader::new("Location").default_open(true).show(ui, |ui| self.location_ui(ui));
        CollapsingHeader::new("Navigation").default_open(true).show(ui, |ui| self.navigation_ui(ui));
        CollapsingHeader::new("Colour").default_open(true).show(ui, |ui| self.colour_ui(ui));
        CollapsingHeader::new("Rendering").default_open(false).show(ui, |ui| self.rendering_ui(ui));
        CollapsingHeader::new("Export").default_open(false).show(ui, |ui| self.export_ui(ui));
        CollapsingHeader::new("Statistics").default_open(true).show(ui, |ui| self.stats_ui(ui));
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
                    Some(v) => self.view = View { imag_down: self.view.imag_down, ..v },
                    None => self.status = "Could not parse the location.".into(),
                }
            }
            if ui.button("Reset").clicked() {
                self.view = View::default();
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
                self.view.zoom = (self.view.zoom / FloatExp::from(self.zoom_factor)).max(FloatExp::from(0.25));
            }
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
        ui.checkbox(&mut self.settings.interior_detection, "Interior detection")
            .on_hover_text("Stop iterating pixels that are attracted to a cycle (big win inside minibrots)");
        ui.checkbox(&mut self.settings.periodic_reference, "Snap reference to nearby minibrot").on_hover_text(
            "Use the nucleus of a minibrot near the view as the reference: its orbit stops after one period",
        );
        ui.checkbox(&mut self.settings.reuse_reference, "Reuse reference orbit while nearby");
        ui.checkbox(&mut self.view.imag_down, "Imaginary axis points down")
            .on_hover_text("The default for .kfr files without ImagPointsUp");
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
        if ui.add_enabled(self.blocking.is_none(), egui::Button::new("Export PNG…")).clicked() {
            self.export_dialog();
        }
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
            row("Total", format!("{:.3} s", st.total.as_secs_f64()));
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

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open location…  (Ctrl+O)").clicked() {
                    self.open_dialog();
                }
                if ui.button("Save location…  (Ctrl+S)").clicked() {
                    self.save_dialog();
                }
                if ui.button("Export PNG…  (Ctrl+E)").clicked() {
                    self.export_dialog();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Controls").clicked() {
                    self.show_help = true;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!("Zoom {:.4}", self.view.zoom));
            ui.separator();
            ui.label(format!("{} iterations", self.view.maxiter));
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
                ui.add(egui::ProgressBar::new(frac as f32).desired_width(180.0).text(label));
                if ui.small_button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.worker.cancel();
                    self.blocking = None;
                    // Do not immediately resubmit the cancelled render.
                    self.requested = Some((self.view.clone(), self.settings.clone()));
                }
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else {
                ui.label(&self.status);
            }
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
            ui.label("Ctrl+O / Ctrl+S / Ctrl+E — open, save location, export PNG");
        });
    }

    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Location", &["kfr", "png"]).pick_file() else { return };
        self.open_path(&path);
    }

    pub fn open_path(&mut self, path: &Path) {
        match kfr::load(path) {
            Ok(loc) => {
                self.view = loc.view;
                self.colouring = loc.colouring;
                self.recolour = true;
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
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(path) = dropped.first() {
            self.open_path(path);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_worker(&ctx);
        self.shortcuts(&ctx);
        egui::Panel::top("menu").show(ui, |ui| self.menu(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("controls").default_size(340.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| self.canvas(ui));
        if self.show_location {
            self.location_window(&ctx);
        }
        if self.show_help {
            self.help_window(&ctx);
        }
        self.request_render();
        if self.recolour {
            ctx.request_repaint();
        }
        let settled = self.worker.current().is_none() && self.shown.as_ref().is_some_and(|s| !s.preview);
        if settled && let Some(path) = self.screenshot_to.take() {
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
