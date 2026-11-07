//! The screens. A first install: one screen of choices, progress, done. An
//! update: one screen with Update, progress, then Fracst opens and Setup
//! closes.

use crate::install::{self, Args, Choices, Existing, FILES, Job, Stage, VERSION};
use crate::look::{self, Kind};
use eframe::egui::{self, Align2, Color32, Key, Pos2, Rect, Sense, TextureHandle, Vec2, ViewportCommand};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

enum Screen {
    Ready,
    Working(Arc<Mutex<Job>>),
    Done(PathBuf),
    Failed(install::Failure),
}

pub struct App {
    args: Args,
    existing: Option<Existing>,
    screen: Screen,
    dir: PathBuf,
    desktop: bool,
    path: bool,
    kfr: bool,
    icon: Option<TextureHandle>,
    /// The progress bar's shown fraction, eased towards the real one.
    shown: f32,
    started: Instant,
}

const MARGIN: f32 = 48.0;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        look::install_fonts(&cc.egui_ctx);
        let icon =
            fracst_core::image::decode_rgba_png(include_bytes!("../../../assets/icon.png")).ok().map(|(rgba, w, h)| {
                let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                cc.egui_ctx.load_texture("icon", image, egui::TextureOptions::LINEAR)
            });
        let existing = install::find_existing();
        let dir = args.dir.clone().unwrap_or_else(|| install::default_dir(args.all_users == Some(true)));
        App {
            args,
            existing,
            screen: Screen::Ready,
            dir,
            desktop: false,
            path: true,
            kfr: true,
            icon,
            shown: 0.0,
            started: Instant::now(),
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let choices = match self.existing {
            Some(_) => None,
            None => Some(Choices { dir: self.dir.clone(), desktop: self.desktop, path: self.path, kfr: self.kfr }),
        };
        self.shown = 0.0;
        self.screen = Screen::Working(install::start(&self.args, self.existing.clone(), choices, ctx.clone()));
    }

    /// The heading and the paragraph under it, for the first screen.
    fn intro(&self) -> (String, String) {
        match &self.existing {
            None => (
                "Install Fracst".into(),
                "A fast deep-zoom explorer for the Mandelbrot set. Zoom past 10¹⁰⁰⁰, and make videos of the way down.".into(),
            ),
            Some(e) => match e.order() {
                Ordering::Greater => (
                    format!("Fracst {VERSION} is ready"),
                    format!("You have {}. Updating closes Fracst, installs {VERSION} and opens it again. Your settings and video projects are kept.", e.version),
                ),
                Ordering::Equal => (
                    format!("Reinstall Fracst {VERSION}"),
                    "This version is already installed. Reinstalling closes Fracst, replaces its files and opens it again.".into(),
                ),
                Ordering::Less => (
                    format!("Go back to Fracst {VERSION}"),
                    format!("You have {}, which is newer. This replaces it with {VERSION}, then opens Fracst again.", e.version),
                ),
            },
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        look::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let window = ui.max_rect();
        look::background(ui, window);

        // The top strip moves the window.
        let strip = Rect::from_min_size(window.min, Vec2::new(window.width(), 48.0));
        let drag = ui.interact(strip, egui::Id::unique("strip"), Sense::drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let busy = matches!(self.screen, Screen::Working(_));
        if !busy && (look::close_button(ui, window).clicked() || ui.input(|i| i.key_pressed(Key::Escape))) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // The icon, with a glow behind it, then the heading.
        let left = window.left() + MARGIN;
        let icon_rect = Rect::from_min_size(Pos2::new(left, window.top() + 52.0), Vec2::splat(60.0));
        look::glow(ui.painter(), icon_rect.center(), 110.0, Color32::from_rgba_unmultiplied(140, 72, 238, 40));
        if let Some(icon) = &self.icon {
            ui.painter().image(
                icon.id(),
                icon_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let text_width = 400.0;
        let heading_y = icon_rect.bottom() + 26.0;
        let heading = |ui: &egui::Ui, text: &str| {
            ui.painter().text(Pos2::new(left, heading_y), Align2::LEFT_TOP, text, look::strong(26.0), look::TEXT);
        };
        let paragraph = |ui: &egui::Ui, text: &str, y: f32| -> f32 {
            let galley = ui.painter().layout(text.to_owned(), look::regular(14.5), look::TEXT_WEAK, text_width);
            let h = galley.size().y;
            ui.painter().galley(Pos2::new(left, y), galley, look::TEXT_WEAK);
            y + h
        };
        let body_y = heading_y + 44.0;
        let buttons_y = window.bottom() - MARGIN - 40.0 + 8.0;
        let right = window.right() - MARGIN;
        let enter = ui.input(|i| i.key_pressed(Key::Enter));

        match &self.screen {
            Screen::Ready => {
                let (title, text) = self.intro();
                heading(ui, &title);
                let below = paragraph(ui, &text, body_y);
                if let Some(existing) = &self.existing {
                    if existing.order() == Ordering::Greater {
                        let notes = look::link(ui, Pos2::new(left, below + 14.0), "What's new in this version");
                        if notes.clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(format!(
                                "https://github.com/alexcrisan1611/fracst/releases/tag/v{VERSION}"
                            )));
                        }
                    }
                } else {
                    // Where, and what else.
                    let mut y = below + 26.0;
                    let field = Rect::from_min_size(Pos2::new(left, y), Vec2::new(text_width + 64.0, 42.0));
                    let dir = self.dir.display().to_string();
                    if look::folder_field(ui, field, &dir).clicked() {
                        // Open at the nearest folder that exists: the install
                        // folder itself usually doesn't yet.
                        let start = self.dir.ancestors().find(|p| p.is_dir()).unwrap_or(&self.dir).to_path_buf();
                        if let Some(picked) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                            // A new folder of its own inside the one picked,
                            // unless the user picked a fracst folder.
                            let named = picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case("fracst"));
                            self.dir = if named { picked } else { picked.join("fracst") };
                        }
                    }
                    y = field.bottom() + 18.0;
                    let rows = [
                        ("Add fracst-cli to PATH", &mut self.path),
                        ("Open .kfr location files with Fracst", &mut self.kfr),
                        ("Desktop shortcut", &mut self.desktop),
                    ];
                    for (label, on) in rows {
                        let row = Rect::from_min_size(Pos2::new(left, y), Vec2::new(text_width, 34.0));
                        look::switch_row(ui, row, label, on);
                        y += 36.0;
                    }
                }
                let primary = if self.existing.is_none() {
                    "Install"
                } else if self.existing.as_ref().is_some_and(|e| e.order() == Ordering::Equal) {
                    "Reinstall"
                } else {
                    "Update"
                };
                let go = look::button(ui, right, buttons_y, primary, Kind::Primary, 128.0);
                let cancel_right = go.rect.left() - 10.0;
                let cancel_label = if self.existing.is_some() { "Not now" } else { "Cancel" };
                if look::button(ui, cancel_right, buttons_y, cancel_label, Kind::Quiet, 96.0).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                if go.clicked() || enter {
                    self.start(&ctx);
                }
            }

            Screen::Working(job) => {
                let (stage, files, result) = {
                    let j = job.lock().unwrap();
                    (j.stage, j.files, j.result.clone())
                };
                heading(ui, if self.existing.is_some() { "Updating Fracst" } else { "Installing Fracst" });
                let what = match stage {
                    Some(Stage::Closing) => "Closing Fracst…",
                    Some(Stage::Finishing) => "Finishing…",
                    _ => "Copying files…",
                };
                let below = paragraph(ui, what, body_y);
                let target = match stage {
                    Some(Stage::Closing) | None => 0.06,
                    _ => 0.1 + 0.85 * (files as f32 / FILES as f32).min(1.0),
                };
                let target = if result.is_some() { 1.0 } else { target };
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                self.shown += (target - self.shown) * (1.0 - (-dt * 6.0).exp());
                let bar = Rect::from_min_size(Pos2::new(left, below + 26.0), Vec2::new(text_width + 64.0, 6.0));
                look::progress(ui, bar, self.shown, self.started.elapsed().as_secs_f64());
                ctx.request_repaint();

                if let Some(result) = result {
                    match result {
                        Ok(dir) if self.existing.is_some() => {
                            // An update ends by opening the new Fracst.
                            install::launch(&dir);
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                            self.screen = Screen::Done(dir);
                        }
                        Ok(dir) => self.screen = Screen::Done(dir),
                        Err(failure) => self.screen = Screen::Failed(failure),
                    }
                }
            }

            Screen::Done(dir) => {
                let dir = dir.clone();
                heading(ui, "Fracst is installed");
                paragraph(ui, "Scroll to zoom in anywhere. Ctrl+K finds everything else.", body_y);
                let open = look::button(ui, right, buttons_y, "Open Fracst", Kind::Primary, 128.0);
                let close_right = open.rect.left() - 10.0;
                if look::button(ui, close_right, buttons_y, "Close", Kind::Quiet, 96.0).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                if open.clicked() || enter {
                    install::launch(&dir);
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }

            Screen::Failed(failure) => {
                let failure = failure.clone();
                heading(ui, "Fracst wasn't installed");
                paragraph(ui, &failure.message, body_y);
                let close = look::button(ui, right, buttons_y, "Close", Kind::Primary, 110.0);
                if let Some(log) = &failure.log {
                    let log_right = close.rect.left() - 10.0;
                    if look::button(ui, log_right, buttons_y, "Open log", Kind::Quiet, 96.0).clicked() {
                        let _ = std::process::Command::new("notepad.exe").arg(log).spawn();
                    }
                }
                if close.clicked() || enter {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }
}
