//! Everything the explorer can do, as one list. The toolbar, the keyboard
//! shortcuts and the Ctrl+K palette all run these, so nothing the app offers
//! is missing from the palette.

use super::App;
use crate::prefs;
use crate::search::{Entry, State};
use crate::video::VideoCmd;
use eframe::egui::{self, Align, Stroke, StrokeKind, ViewportCommand};
use fracst_core::{Slopes, Transfer, View, kfr};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Open,
    Save,
    Export,
    CopyLocation,
    PasteLocation,
    MakeVideo,
    OpenVideoProject,
    Quit,
    ZoomIn,
    ZoomOut,
    ResetView,
    FindMinibrot,
    CancelRender,
    ZoomFactor(f64),
    Glide(u32),
    RandomPalette,
    Palette(usize),
    Transfer(Transfer),
    Resolution(f32),
    Toggle(Flag),
    Focus(Spot),
    Connect,
    Disconnect,
    TogglePanel,
    Controls,
    Minimise,
    Maximise,
    Video(VideoCmd),
}

/// On/off settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flag {
    AutoIterations,
    Smooth,
    Flat,
    Slopes,
    Bla,
    Simd,
    InteriorDetection,
    SnapReference,
    ReuseReference,
    ImagDown,
    Region,
    UpdateCheck,
}

impl Flag {
    const ALL: [(Flag, &'static str, &'static str, &'static str); 12] = [
        (Flag::AutoIterations, "Auto iterations", "Navigation", "auto-iterations"),
        (Flag::Smooth, "Smooth colouring", "Colour", "smooth"),
        (Flag::Flat, "Flat colouring", "Colour", "flat"),
        (Flag::Slopes, "Slope shading", "Colour", "slopes"),
        (Flag::Bla, "Bilinear approximation (BLA)", "Rendering", "bla"),
        (Flag::Simd, "SIMD (AVX2)", "Rendering", "simd"),
        (Flag::InteriorDetection, "Interior detection", "Rendering", "interior-detection"),
        (Flag::SnapReference, "Snap reference to nearby minibrot", "Rendering", "snap-reference"),
        (Flag::ReuseReference, "Reuse reference orbit while nearby", "Rendering", "reuse-reference"),
        (Flag::ImagDown, "Imaginary axis points down", "Rendering", "imag-down"),
        (Flag::Region, "Render only a region", "Rendering", "region"),
        (Flag::UpdateCheck, "Check for updates at startup", "App", "update-check"),
    ];
}

/// Sections of the settings card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    Location,
    Navigation,
    Colour,
    Rendering,
    Remote,
    Export,
    Statistics,
}

/// Controls in the settings card that the palette can take you to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    Real,
    Imaginary,
    Zoom,
    Iterations,
    MinibrotFill,
    PaletteKeys,
    IterDiv,
    Offset,
    InteriorColour,
    SlopePower,
    SlopeRatio,
    SlopeAngle,
    BlaBits,
    Bailout,
    RegionSize,
    ExportSize,
    Supersample,
    RemoteServer,
    RemoteKey,
    Statistics,
}

impl Spot {
    const ALL: [(Spot, &'static str, &'static str); 20] = [
        (Spot::Real, "Real coordinate", "real"),
        (Spot::Imaginary, "Imaginary coordinate", "imaginary"),
        (Spot::Zoom, "Zoom level", "zoom"),
        (Spot::Iterations, "Iteration limit", "iterations"),
        (Spot::MinibrotFill, "Minibrot fill", "minibrot-fill"),
        (Spot::PaletteKeys, "Palette colours", "palette-colours"),
        (Spot::IterDiv, "Iter div (colour bands)", "iter-div"),
        (Spot::Offset, "Palette offset", "offset"),
        (Spot::InteriorColour, "Interior colour", "interior-colour"),
        (Spot::SlopePower, "Slope power", "slope-power"),
        (Spot::SlopeRatio, "Slope ratio", "slope-ratio"),
        (Spot::SlopeAngle, "Slope angle", "slope-angle"),
        (Spot::BlaBits, "BLA precision bits", "bla-bits"),
        (Spot::Bailout, "Bailout", "bailout"),
        (Spot::RegionSize, "Region size", "region-size"),
        (Spot::ExportSize, "Export size", "export-size"),
        (Spot::Supersample, "Export supersampling", "supersample"),
        (Spot::RemoteServer, "Remote GPU server", "remote-server"),
        (Spot::RemoteKey, "Remote GPU key", "remote-key"),
        (Spot::Statistics, "Render statistics", "statistics"),
    ];

    pub fn section(self) -> Section {
        use Spot::*;
        match self {
            Real | Imaginary | Zoom | Iterations => Section::Location,
            MinibrotFill => Section::Navigation,
            PaletteKeys | IterDiv | Offset | InteriorColour | SlopePower | SlopeRatio | SlopeAngle => Section::Colour,
            BlaBits | Bailout | RegionSize => Section::Rendering,
            ExportSize | Supersample => Section::Export,
            RemoteServer | RemoteKey => Section::Remote,
            Statistics => Section::Statistics,
        }
    }

    fn group(self) -> &'static str {
        match self.section() {
            Section::Location => "Location",
            Section::Navigation => "Navigation",
            Section::Colour => "Colour",
            Section::Rendering => "Rendering",
            Section::Remote => "Remote GPU",
            Section::Export => "Export",
            Section::Statistics => "Statistics",
        }
    }
}

/// A control the palette took you to: its section opens, it scrolls into
/// view, takes focus, and glows for a moment.
pub struct Spotlight {
    pub spot: Spot,
    start: Instant,
    focused: bool,
}

impl Spotlight {
    pub fn new(spot: Spot) -> Spotlight {
        Spotlight { spot, start: Instant::now(), focused: false }
    }

    /// Whether this section should be forced open.
    pub fn opens(&self, section: Section) -> bool {
        self.spot.section() == section && self.start.elapsed().as_secs_f32() < 0.5
    }
}

const GLOW_SECS: f32 = 1.6;

/// Call after adding the control at `here`.
pub fn spot(ui: &egui::Ui, response: &egui::Response, here: Spot, light: &mut Option<Spotlight>) {
    let Some(l) = light else { return };
    if l.spot != here {
        return;
    }
    let age = l.start.elapsed().as_secs_f32();
    if age < 0.5 {
        // The section may still be opening: keep it in view until it has.
        response.scroll_to_me(Some(Align::Center));
    }
    if !l.focused {
        response.request_focus();
        l.focused = true;
    }
    let glow = 1.0 - age / GLOW_SECS;
    if glow <= 0.0 {
        *light = None;
        return;
    }
    let colour = ui.visuals().hyperlink_color.gamma_multiply(glow);
    ui.painter().rect_stroke(response.rect.expand(3.0), 6, Stroke::new(2.0, colour), StrokeKind::Outside);
    ui.ctx().request_repaint();
}

/// "Ctrl+O", or "Cmd+O" on a Mac.
fn keys(k: &str) -> String {
    if cfg!(target_os = "macos") { format!("Cmd+{k}") } else { format!("Ctrl+{k}") }
}

impl App {
    /// Everything the palette lists, as things stand.
    pub(super) fn entries(&self) -> Vec<Entry<Cmd>> {
        let mut out = Vec::new();
        let mut add = |cmd: Cmd, title: String, group: &'static str, key: String, shortcut: Option<String>| {
            out.push(Entry { action: cmd, title, group, shortcut, state: State::Action, key });
        };
        use Cmd::*;
        add(Open, "Open location or video project…".into(), "File", "open".into(), Some(keys("O")));
        add(Save, "Save location…".into(), "File", "save".into(), Some(keys("S")));
        add(Export, "Export PNG…".into(), "File", "export".into(), Some(keys("E")));
        add(CopyLocation, "Copy location as text".into(), "File", "copy".into(), None);
        add(PasteLocation, "Paste location text…".into(), "File", "paste".into(), None);
        add(MakeVideo, "Make video…".into(), "Video", "make-video".into(), None);
        add(OpenVideoProject, "Open video project…".into(), "Video", "open-video".into(), None);
        for (cmd, title) in self.video.commands() {
            add(Video(cmd), title.into(), "Video", format!("video:{cmd:?}"), None);
        }
        add(ZoomIn, "Zoom in".into(), "Navigation", "zoom-in".into(), Some("Click".into()));
        add(ZoomOut, "Zoom out".into(), "Navigation", "zoom-out".into(), Some("Right click".into()));
        add(ResetView, "Reset view to the whole set".into(), "Navigation", "reset".into(), None);
        add(FindMinibrot, "Find nearest minibrot".into(), "Navigation", "minibrot".into(), None);
        if self.worker.current().is_some() {
            add(CancelRender, "Cancel the render".into(), "Navigation", "cancel".into(), Some("Esc".into()));
        }
        add(RandomPalette, "Random palette".into(), "Colour", "random-palette".into(), None);
        add(TogglePanel, "Show or hide settings".into(), "App", "panel".into(), Some("Tab".into()));
        add(Controls, "Controls and shortcuts".into(), "App", "controls".into(), None);
        let connected = !matches!(self.remote_status, crate::worker::RemoteStatus::Off);
        if connected {
            add(Disconnect, "Disconnect the remote GPU".into(), "Remote GPU", "disconnect".into(), None);
        } else {
            add(Connect, "Connect to the remote GPU".into(), "Remote GPU", "connect".into(), None);
        }
        if !crate::window::NATIVE_BUTTONS {
            add(Minimise, "Minimise window".into(), "Window", "minimise".into(), None);
            add(Maximise, "Maximise or restore window".into(), "Window", "maximise".into(), None);
        }
        add(Quit, "Quit".into(), "App", "quit".into(), None);

        // Settings: switches and choices change here; the rest open the settings.
        let mut choice = |cmd: Cmd, title: String, group: &'static str, key: String, on: bool| {
            let state = if on { State::On } else { State::Off };
            out.push(Entry { action: cmd, title, group, shortcut: None, state, key });
        };
        for (flag, title, group, key) in Flag::ALL {
            choice(Toggle(flag), title.into(), group, format!("toggle:{key}"), self.flag(flag));
        }
        for f in [2.0, 4.0, 8.0, 16.0, 32.0] {
            let on = self.zoom_factor == f;
            choice(ZoomFactor(f), format!("Click zoom ×{f}"), "Navigation", format!("zoom-factor:{f}"), on);
        }
        for (name, ms) in prefs::GLIDES {
            let on = self.prefs.glide_ms == ms;
            choice(Glide(ms), format!("Zoom animation: {name}"), "Navigation", format!("glide:{ms}"), on);
        }
        for (i, (name, p)) in fracst_core::Palette::presets().into_iter().enumerate() {
            let on = self.colouring.palette == p;
            choice(Palette(i), format!("Palette: {name}"), "Colour", format!("palette:{name}"), on);
        }
        for t in fracst_core::Transfer::ALL {
            let on = self.colouring.transfer == t;
            choice(Transfer(t), format!("Transfer: {}", t.name()), "Colour", format!("transfer:{}", t.name()), on);
        }
        for s in [0.25f32, 0.5, 1.0, 2.0] {
            let on = self.resolution_scale == s;
            let title = format!("Resolution: {:.0}%", s * 100.0);
            choice(Resolution(s), title, "Rendering", format!("resolution:{s}"), on);
        }
        for (spot, title, key) in Spot::ALL {
            out.push(Entry {
                action: Focus(spot),
                title: title.into(),
                group: spot.group(),
                shortcut: None,
                state: State::Action,
                key: format!("focus:{key}"),
            });
        }
        out
    }

    fn flag(&self, flag: Flag) -> bool {
        let (s, c) = (&self.settings, &self.colouring);
        match flag {
            Flag::AutoIterations => s.auto_iterations,
            Flag::Smooth => c.smooth,
            Flag::Flat => c.flat,
            Flag::Slopes => c.slopes.is_some(),
            Flag::Bla => s.bla,
            Flag::Simd => s.simd,
            Flag::InteriorDetection => s.interior_detection,
            Flag::SnapReference => s.periodic_reference,
            Flag::ReuseReference => s.reuse_reference,
            Flag::ImagDown => self.view.imag_down,
            Flag::Region => self.region.is_some(),
            Flag::UpdateCheck => self.update_check,
        }
    }

    fn set_flag(&mut self, flag: Flag, on: bool) {
        let (s, c) = (&mut self.settings, &mut self.colouring);
        match flag {
            Flag::AutoIterations => s.auto_iterations = on,
            Flag::Smooth => c.smooth = on,
            Flag::Flat => c.flat = on,
            Flag::Slopes => c.slopes = on.then(|| c.slopes.unwrap_or_default()),
            Flag::Bla => s.bla = on,
            Flag::Simd => s.simd = on,
            Flag::InteriorDetection => s.interior_detection = on,
            Flag::SnapReference => s.periodic_reference = on,
            Flag::ReuseReference => s.reuse_reference = on,
            Flag::ImagDown => self.view.imag_down = on,
            Flag::Region => {
                self.region = on.then_some(self.region_size);
                self.region_changed = true;
            }
            Flag::UpdateCheck => {
                self.update_check = on;
                crate::update::set_enabled(on);
            }
        }
        self.recolour = true;
    }

    pub(super) fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        use Cmd::*;
        match cmd {
            Open => self.open_dialog(),
            Save => self.save_dialog(),
            Export => {
                if self.blocking.is_none() {
                    self.export_dialog();
                }
            }
            CopyLocation => {
                ctx.copy_text(kfr::to_string(&self.view, &self.colouring));
                self.status = "Location copied to the clipboard.".into();
            }
            PasteLocation => {
                self.location_text.clear();
                self.show_location = true;
            }
            MakeVideo => self.make_video(),
            OpenVideoProject => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Video project folder").pick_folder() {
                    self.open_video_project(&dir);
                }
            }
            Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            ZoomIn => self.zoom_in_at(self.settings.width as f64 / 2.0, self.settings.height as f64 / 2.0),
            ZoomOut => self.zoom_out(),
            ResetView => {
                self.view = View { imag_down: self.view.imag_down, ..View::default() };
                self.jump = true;
            }
            FindMinibrot => self.find_minibrot(),
            CancelRender => self.cancel_render(),
            ZoomFactor(f) => self.zoom_factor = f,
            Glide(ms) => {
                self.prefs.glide_ms = ms;
                self.prefs.save();
            }
            RandomPalette => self.random_palette(),
            Palette(i) => {
                if let Some((_, p)) = fracst_core::Palette::presets().into_iter().nth(i) {
                    self.colouring.palette = p;
                    self.recolour = true;
                }
            }
            Transfer(t) => {
                self.colouring.transfer = t;
                self.recolour = true;
            }
            Resolution(s) => self.resolution_scale = s,
            Toggle(flag) => self.set_flag(flag, !self.flag(flag)),
            Focus(spot) => {
                if matches!(spot, Spot::SlopePower | Spot::SlopeRatio | Spot::SlopeAngle)
                    && self.colouring.slopes.is_none()
                {
                    self.colouring.slopes = Some(Slopes::default());
                    self.recolour = true;
                }
                if !self.prefs.panel {
                    self.toggle_panel();
                }
                self.spotlight = Some(Spotlight::new(spot));
            }
            Connect => {
                if self.remote_url.trim().is_empty() {
                    self.run(ctx, Focus(Spot::RemoteServer));
                } else {
                    self.connect_remote();
                }
            }
            Disconnect => self.disconnect_remote(),
            TogglePanel => self.toggle_panel(),
            Controls => self.show_help = true,
            Minimise => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            Maximise => {
                let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
            }
            Video(cmd) => {
                let remote = self.remote();
                self.video.run(cmd, remote);
            }
        }
    }
}
