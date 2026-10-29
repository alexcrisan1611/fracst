//! The look: bundled fonts, rounded, translucent surfaces in one of three
//! schemes (Dark, Midnight and Light), an accent colour taken from the
//! current palette, and on the dark schemes, a darker fractal.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    ecolor::Hsva,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Corner radius of the window, the canvas card and the floating cards.
pub const RADIUS: u8 = 12;
/// Gap between the window's edge and the canvas card.
pub const GAP: f32 = 8.0;

/// The look the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
    /// Dark or Light, as the system is.
    System,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::Dark, Appearance::Light, Appearance::System];

    pub fn name(self) -> &'static str {
        match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
            Appearance::System => "Match system",
        }
    }

    /// How it is saved in the settings and named in the palette's keys.
    pub fn key(self) -> &'static str {
        match self {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
            Appearance::System => "system",
        }
    }

    pub fn from_key(key: &str) -> Option<Appearance> {
        Appearance::ALL.into_iter().find(|a| a.key() == key)
    }

    /// The scheme to show, given the system's theme if it is known; when it
    /// is dark, `midnight` makes it true black.
    pub fn scheme(self, system: Option<egui::Theme>, midnight: bool) -> Scheme {
        let light = match self {
            Appearance::Dark => false,
            Appearance::Light => true,
            Appearance::System => system == Some(egui::Theme::Light),
        };
        match (light, midnight) {
            (true, _) => Scheme::Light,
            (false, true) => Scheme::Midnight,
            (false, false) => Scheme::Dark,
        }
    }
}

/// A set of colours for the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Charcoal cards over a near-black window.
    Dark,
    /// Dark, but true black, with the cards barely lifted from it: for dim
    /// rooms and OLED screens.
    Midnight,
    /// Frosted white cards over a pale grey window.
    Light,
}

impl Scheme {
    fn colours(self) -> &'static Colours {
        match self {
            Scheme::Dark => &DARK,
            Scheme::Midnight => &MIDNIGHT,
            Scheme::Light => &LIGHT,
        }
    }

    pub fn is_dark(self) -> bool {
        self != Scheme::Light
    }
}

pub struct Colours {
    /// Behind the canvas card.
    pub window: Color32,
    /// Floating cards and pills: slightly see-through over the fractal.
    pub card: Color32,
    /// For what needs reading over a busy fractal, like the palette.
    pub solid: Color32,
    pub card_stroke: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    /// Text and icons on a hovered or pressed control.
    pub text_strong: Color32,
    /// A faint wash over hovered rows and icon buttons.
    pub wash: Color32,
    /// Switches that are off, and the outlines of key caps.
    pub muted: Color32,
    pub separator: Color32,
    /// Text fields and progress bars.
    pub field: Color32,
    pub code: Color32,
    /// Alternate rows in tables.
    pub stripe: Color32,
    /// Buttons, as (fill, weak fill): at rest, hovered, and with their menu open.
    pub button: (Color32, Color32),
    pub button_hovered: (Color32, Color32),
    pub button_open: (Color32, Color32),
    pub warn: Color32,
    pub error: Color32,
    /// How dark the shadows are, from 0 to 1.
    pub shadow: f32,
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(r, g, b, a)
}

const DARK: Colours = Colours {
    window: Color32::from_rgb(11, 11, 14),
    card: rgba(17, 17, 21, 224),
    solid: rgba(19, 19, 24, 248),
    card_stroke: rgba(56, 56, 64, 140),
    text: Color32::from_rgb(226, 226, 232),
    text_weak: Color32::from_rgb(150, 150, 160),
    text_strong: Color32::WHITE,
    wash: rgba(8, 8, 8, 8),
    muted: Color32::from_gray(70),
    separator: rgba(60, 60, 68, 120),
    field: rgba(6, 6, 9, 200),
    code: Color32::from_rgb(24, 24, 30),
    stripe: rgba(6, 6, 6, 6),
    button: (rgba(48, 48, 56, 200), rgba(38, 38, 46, 190)),
    button_hovered: (rgba(62, 62, 72, 220), rgba(56, 56, 66, 220)),
    button_open: (rgba(56, 56, 66, 230), rgba(48, 48, 58, 230)),
    warn: Color32::from_rgb(230, 170, 90),
    error: Color32::from_rgb(230, 110, 90),
    shadow: 1.0,
};

const MIDNIGHT: Colours = Colours {
    window: Color32::BLACK,
    card: rgba(5, 5, 7, 232),
    solid: rgba(7, 7, 9, 250),
    card_stroke: rgba(40, 40, 48, 150),
    text: Color32::from_rgb(232, 232, 238),
    text_weak: Color32::from_rgb(138, 138, 150),
    text_strong: Color32::WHITE,
    wash: rgba(9, 9, 9, 9),
    muted: Color32::from_gray(54),
    separator: rgba(44, 44, 52, 130),
    field: rgba(15, 15, 19, 230),
    code: Color32::from_rgb(14, 14, 18),
    stripe: rgba(5, 5, 5, 5),
    button: (rgba(32, 32, 38, 210), rgba(24, 24, 30, 200)),
    button_hovered: (rgba(46, 46, 54, 225), rgba(40, 40, 48, 225)),
    button_open: (rgba(40, 40, 48, 235), rgba(34, 34, 42, 235)),
    warn: Color32::from_rgb(230, 170, 90),
    error: Color32::from_rgb(230, 110, 90),
    shadow: 1.0,
};

const LIGHT: Colours = Colours {
    window: Color32::from_rgb(228, 228, 234),
    // Bright fractals show through white far more than through charcoal, so
    // the light cards are more opaque than the dark ones.
    card: Color32::from_rgba_unmultiplied_const(250, 250, 252, 244),
    solid: Color32::from_rgba_unmultiplied_const(252, 252, 254, 252),
    card_stroke: Color32::from_black_alpha(34),
    text: Color32::from_rgb(28, 28, 34),
    text_weak: Color32::from_rgb(100, 100, 112),
    text_strong: Color32::BLACK,
    wash: Color32::from_black_alpha(14),
    muted: Color32::from_gray(190),
    separator: Color32::from_black_alpha(28),
    field: Color32::from_rgba_unmultiplied_const(222, 222, 230, 245),
    code: Color32::from_rgb(232, 232, 238),
    stripe: Color32::from_black_alpha(7),
    button: (Color32::from_black_alpha(30), Color32::from_black_alpha(20)),
    button_hovered: (Color32::from_black_alpha(44), Color32::from_black_alpha(34)),
    button_open: (Color32::from_black_alpha(38), Color32::from_black_alpha(30)),
    warn: Color32::from_rgb(170, 96, 0),
    error: Color32::from_rgb(190, 50, 34),
    shadow: 0.35,
};

/// The scheme showing, as an index into `Scheme`'s variants.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The colours of the scheme showing.
pub fn colours() -> &'static Colours {
    match CURRENT.load(Ordering::Relaxed) {
        1 => &MIDNIGHT,
        2 => &LIGHT,
        _ => &DARK,
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("inter", include_bytes!("../../../assets/fonts/Inter-Regular.ttf"));
    add("inter-semibold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
    add("mono", include_bytes!("../../../assets/fonts/JetBrainsMonoNL-Regular.ttf"));
    // The defaults stay behind ours as fallbacks, for symbols and emoji.
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.insert(
        FontFamily::Name(STRONG.into()),
        std::iter::once("inter-semibold".to_string()).chain(fallbacks).collect(),
    );
    ctx.set_fonts(fonts);
}

const STRONG: &str = "strong";

/// The semibold face, for headings and the wordmark.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Set the fonts and spacing once at startup, then the colours; [`apply`]
/// recolours it later.
pub fn install(ctx: &egui::Context, scheme: Scheme, accent: Color32) {
    install_fonts(ctx);
    ctx.all_styles_mut(|style| {
        use FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Heading, strong(17.0)),
            (TextStyle::Monospace, FontId::new(12.5, Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(9.0, 4.0);
        s.interact_size.y = 24.0;
        s.window_margin = Margin::same(14);
        s.menu_margin = Margin::same(8);
        s.slider_rail_height = 4.0;
        s.combo_width = 120.0;
        s.indent = 14.0;
    });
    apply(ctx, scheme, accent);
}

/// Show a scheme, with its highlights in `accent`.
pub fn apply(ctx: &egui::Context, scheme: Scheme, accent: Color32) {
    CURRENT.store(scheme as u8, Ordering::Relaxed);
    let theme = if scheme.is_dark() { egui::Theme::Dark } else { egui::Theme::Light };
    // Pinned, not left to egui: Match system is resolved by `Appearance::scheme`.
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, visuals(scheme, accent));
}

fn visuals(scheme: Scheme, accent: Color32) -> egui::Visuals {
    let c = scheme.colours();
    let mut v = if scheme.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(7);
    let shadow = |a: f32| Color32::from_black_alpha((a * c.shadow) as u8);
    v.override_text_color = None;
    v.panel_fill = c.window;
    v.window_fill = c.card;
    v.window_stroke = Stroke::new(1.0, c.card_stroke);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: shadow(110.0) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: shadow(100.0) };
    v.extreme_bg_color = c.field;
    v.text_edit_bg_color = Some(c.field);
    v.faint_bg_color = c.stripe;
    v.code_bg_color = c.code;
    v.warn_fg_color = c.warn;
    v.error_fg_color = c.error;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.gamma_multiply(if scheme.is_dark() { 0.85 } else { 0.5 });
    v.selection.stroke = Stroke::new(1.0, if scheme.is_dark() { on(accent) } else { c.text_strong });
    v.text_cursor.stroke = Stroke::new(2.0, accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, c.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.text);
    w.noninteractive.corner_radius = r;

    (w.inactive.bg_fill, w.inactive.weak_bg_fill) = c.button;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, c.text);
    w.inactive.corner_radius = r;

    (w.hovered.bg_fill, w.hovered.weak_bg_fill) = c.button_hovered;
    w.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.6));
    w.hovered.fg_stroke = Stroke::new(1.5, c.text_strong);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = accent.gamma_multiply(if scheme.is_dark() { 0.55 } else { 0.4 });
    w.active.weak_bg_fill = accent.gamma_multiply(if scheme.is_dark() { 0.45 } else { 0.3 });
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.active.fg_stroke = Stroke::new(1.5, c.text_strong);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    (w.open.bg_fill, w.open.weak_bg_fill) = c.button_open;
    w.open.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.5));
    w.open.fg_stroke = Stroke::new(1.0, c.text);
    w.open.corner_radius = r;
    v
}

/// Black or white, whichever reads better on `bg`.
fn on(bg: Color32) -> Color32 {
    if luminance(bg) > 0.45 { Color32::from_rgb(12, 12, 16) } else { Color32::WHITE }
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// The accent for a palette: its most vivid key colour, brought into a range
/// that reads on the scheme's cards whatever the palette is.
pub fn accent_from(keys: &[[u8; 3]], scheme: Scheme) -> Color32 {
    let best = keys
        .iter()
        .map(|&k| Hsva::from_srgb(k))
        // Vivid: saturated and not too dark.
        .max_by(|a, b| (a.s * a.v.sqrt()).total_cmp(&(b.s * b.v.sqrt())));
    let rgb = |h: Hsva| {
        let [r, g, b] = h.to_srgb();
        Color32::from_rgb(r, g, b)
    };
    match best {
        Some(h) if h.s * h.v.sqrt() >= 0.18 => {
            if scheme.is_dark() {
                rgb(Hsva { h: h.h, s: h.s.clamp(0.45, 0.85), v: h.v.clamp(0.82, 1.0), a: 1.0 })
            } else {
                // Deep enough to read on white: darken until it does.
                let mut a = Hsva { h: h.h, s: h.s.clamp(0.6, 0.9), v: h.v.clamp(0.5, 0.85), a: 1.0 };
                while luminance(rgb(a)) > LIGHT_ACCENT_LUMINANCE && a.v > 0.1 {
                    a.v -= 0.02;
                }
                rgb(a)
            }
        }
        _ if scheme.is_dark() => Color32::from_rgb(170, 180, 200),
        _ => Color32::from_rgb(78, 88, 110),
    }
}

/// At most this luminance for accents on light cards: at least 3.5:1 against white.
const LIGHT_ACCENT_LUMINANCE: f32 = 0.25;

/// Darken a frame, as RGBA bytes, for showing on a dark scheme; only what is
/// on screen, not what is saved. Pale, washed-out colours like a palette's
/// white lose most of their white and become deep tints of their hue;
/// saturated colours keep their colour, and what is still bright is toned
/// down, so nothing glares in a dark room. Midnight goes deeper still.
pub fn darken(rgba: &mut [u8], scheme: Scheme) {
    use rayon::prelude::*;
    let d = match scheme {
        Scheme::Dark => &DARK_IMAGE,
        Scheme::Midnight => &MIDNIGHT_IMAGE,
        Scheme::Light => return,
    };
    rgba.par_chunks_mut(4 * 4096).for_each(|chunk| {
        for [r, g, b, _] in chunk.as_chunks_mut::<4>().0 {
            [*r, *g, *b] = d.pixel([*r, *g, *b]);
        }
    });
}

struct Darkening {
    /// How much of the white in a colour goes.
    white: f32,
    /// How much the brightest colours are toned down, in proportion; at most
    /// 0.5, or brighter would come out darker.
    highlights: f32,
    /// Everything is scaled by this.
    level: f32,
}

const DARK_IMAGE: Darkening = Darkening { white: 0.7, highlights: 0.5, level: 0.9 };
const MIDNIGHT_IMAGE: Darkening = Darkening { white: 0.85, highlights: 0.5, level: 0.75 };

impl Darkening {
    fn pixel(&self, rgb: [u8; 3]) -> [u8; 3] {
        let white = rgb.iter().copied().min().unwrap_or(0) as f32 * self.white;
        let [r, g, b] = rgb.map(|v| (v as f32 - white) / 255.0);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let scale = self.level * (1.0 - self.highlights * luma) * 255.0;
        [r, g, b].map(|v| (v * scale).round() as u8)
    }
}

/// A rounded card for floating controls.
pub fn card() -> egui::Frame {
    let c = colours();
    egui::Frame::new()
        .fill(c.card)
        .stroke(Stroke::new(1.0, c.card_stroke))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .shadow(Shadow {
            offset: [0, 6],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha((90.0 * c.shadow) as u8),
        })
}

/// A small pill for the toolbar and the status line.
pub fn pill() -> egui::Frame {
    card().corner_radius(CornerRadius::same(255)).inner_margin(Margin::symmetric(10, 5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_vivid_and_readable() {
        // A palette of greys falls back to the neutral accent.
        let greys = [[0, 0, 0], [128, 128, 128], [255, 255, 255]];
        assert_eq!(accent_from(&greys, Scheme::Dark), Color32::from_rgb(170, 180, 200));
        // A dark navy palette still gives an accent bright enough for dark cards.
        let navy = [[0, 0, 40], [10, 10, 90], [255, 255, 255]];
        let a = accent_from(&navy, Scheme::Dark);
        assert!(luminance(a) > 0.08, "{a:?}");
        assert!(a.b() > a.r() && a.b() > a.g(), "keeps the hue: {a:?}");
        // The most vivid key wins.
        let a = accent_from(&[[250, 250, 250], [230, 30, 20], [90, 80, 80]], Scheme::Midnight);
        assert!(a.r() > 200 && a.g() < 120, "{a:?}");
    }

    #[test]
    fn light_accent_reads_on_white() {
        // Yellow is the hardest: it is bright at any saturation.
        for keys in [[[255, 255, 0]], [[0, 255, 255]], [[230, 30, 20]], [[0, 0, 90]], [[128, 128, 128]]] {
            let a = accent_from(&keys, Scheme::Light);
            assert!(luminance(a) <= LIGHT_ACCENT_LUMINANCE, "{keys:?} gives {a:?}");
        }
        let a = accent_from(&[[255, 230, 0]], Scheme::Light);
        assert!(a.r() > a.b() && a.g() > a.b(), "keeps the hue: {a:?}");
    }

    #[test]
    fn text_contrasts_in_every_scheme() {
        let contrast = |a: Color32, b: Color32| {
            let (la, lb) = (luminance(a) + 0.05, luminance(b) + 0.05);
            la.max(lb) / la.min(lb)
        };
        for scheme in [Scheme::Dark, Scheme::Midnight, Scheme::Light] {
            let c = scheme.colours();
            // Cards are nearly opaque: judge text against the card over the window.
            let card = c.window.blend(c.card);
            assert!(contrast(c.text, card) >= 7.0, "{scheme:?}");
            assert!(contrast(c.text_weak, card) >= 4.5, "{scheme:?}");
        }
    }

    #[test]
    fn appearance_keys_round_trip() {
        for a in Appearance::ALL {
            assert_eq!(Appearance::from_key(a.key()), Some(a));
        }
        assert_eq!(Appearance::System.scheme(Some(egui::Theme::Light), true), Scheme::Light);
        assert_eq!(Appearance::System.scheme(None, false), Scheme::Dark);
        assert_eq!(Appearance::System.scheme(Some(egui::Theme::Dark), true), Scheme::Midnight);
        assert_eq!(Appearance::Light.scheme(None, true), Scheme::Light);
    }

    #[test]
    fn darkening_tames_pale_colours_and_keeps_hue() {
        let lum = |[r, g, b]: [u8; 3]| luminance(Color32::from_rgb(r, g, b));
        let lilac = [200, 175, 215];
        for d in [&DARK_IMAGE, &MIDNIGHT_IMAGE] {
            // White becomes a dim grey, not black.
            let [r, g, b] = d.pixel([255, 255, 255]);
            assert!(r == g && g == b && (15..90).contains(&r), "{r}");
            // A pale lilac becomes a deep purple: much darker, still purple.
            let [r, g, b] = d.pixel(lilac);
            assert!(lum([r, g, b]) < lum(lilac) / 5.0 && b > r && r > g && r > 25, "{r} {g} {b}");
            // Black stays black, and darkening never brightens.
            assert_eq!(d.pixel([0, 0, 0]), [0, 0, 0]);
            for c in [[0, 128, 192], [255, 255, 0], [30, 30, 30], [255, 0, 0]] {
                assert!(lum(d.pixel(c)) <= lum(c), "{c:?}");
            }
            // Brighter stays brighter, so the image's shading survives.
            let mut last = 0.0;
            for v in (0..=255).step_by(5) {
                let l = lum(d.pixel([v, v / 2, v]));
                assert!(l >= last, "{v}");
                last = l;
            }
        }
        // Midnight is darker than Dark.
        assert!(lum(MIDNIGHT_IMAGE.pixel(lilac)) < lum(DARK_IMAGE.pixel(lilac)) * 0.6);
        // Alpha is left alone, and Light leaves the image alone.
        let mut px = [255, 255, 255, 77];
        darken(&mut px, Scheme::Midnight);
        assert_eq!(px[3], 77);
        let mut px = [255, 255, 255, 77];
        darken(&mut px, Scheme::Light);
        assert_eq!(px, [255, 255, 255, 77]);
    }

    #[test]
    fn text_on_accent_contrasts() {
        assert_eq!(on(Color32::from_rgb(250, 230, 80)), Color32::from_rgb(12, 12, 16));
        assert_eq!(on(Color32::from_rgb(40, 60, 200)), Color32::WHITE);
    }
}
