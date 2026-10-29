//! The look: bundled fonts, a dark theme with rounded, translucent surfaces,
//! and an accent colour taken from the current palette.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    ecolor::Hsva,
};
use std::sync::Arc;

/// Corner radius of the window, the canvas card and the floating cards.
pub const RADIUS: u8 = 12;
/// Gap between the window's edge and the canvas card.
pub const GAP: f32 = 8.0;
/// Behind the canvas card.
pub const WINDOW_BG: Color32 = Color32::from_rgb(11, 11, 14);
/// Floating cards and pills: dark and slightly see-through over the fractal.
pub const CARD_FILL: Color32 = Color32::from_rgba_premultiplied(17, 17, 21, 224);
/// For what needs reading over a busy fractal, like the palette.
pub const SOLID_FILL: Color32 = Color32::from_rgba_premultiplied(19, 19, 24, 248);
pub const CARD_STROKE: Color32 = Color32::from_rgba_premultiplied(56, 56, 64, 140);
pub const TEXT: Color32 = Color32::from_rgb(226, 226, 232);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(150, 150, 160);

/// Used when the palette is all greys.
const NEUTRAL_ACCENT: Color32 = Color32::from_rgb(170, 180, 200);

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

/// Set the theme once at startup; [`set_accent`] recolours it later.
pub fn install(ctx: &egui::Context, accent: Color32) {
    install_fonts(ctx);
    // Dark whatever the system's theme: the cards and pills are dark.
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|style| {
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
        style.visuals = visuals(accent);
    });
}

/// Recolour the theme's highlights.
pub fn set_accent(ctx: &egui::Context, accent: Color32) {
    ctx.global_style_mut(|style| style.visuals = visuals(accent));
}

fn visuals(accent: Color32) -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let r = CornerRadius::same(7);
    v.override_text_color = None;
    v.panel_fill = WINDOW_BG;
    v.window_fill = CARD_FILL;
    v.window_stroke = Stroke::new(1.0, CARD_STROKE);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(110) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(100) };
    v.extreme_bg_color = Color32::from_rgba_premultiplied(6, 6, 9, 200);
    v.faint_bg_color = Color32::from_rgba_premultiplied(255, 255, 255, 6);
    v.code_bg_color = Color32::from_rgb(24, 24, 30);
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.gamma_multiply(0.85);
    v.selection.stroke = Stroke::new(1.0, on(accent));
    v.text_cursor.stroke = Stroke::new(2.0, accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::TRANSPARENT;
    w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgba_premultiplied(60, 60, 68, 120));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = Color32::from_rgba_premultiplied(48, 48, 56, 200);
    w.inactive.weak_bg_fill = Color32::from_rgba_premultiplied(38, 38, 46, 190);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = Color32::from_rgba_premultiplied(62, 62, 72, 220);
    w.hovered.weak_bg_fill = Color32::from_rgba_premultiplied(56, 56, 66, 220);
    w.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.6));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = accent.gamma_multiply(0.55);
    w.active.weak_bg_fill = accent.gamma_multiply(0.45);
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = Color32::from_rgba_premultiplied(56, 56, 66, 230);
    w.open.weak_bg_fill = Color32::from_rgba_premultiplied(48, 48, 58, 230);
    w.open.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.5));
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
/// that reads on the dark cards whatever the palette is.
pub fn accent_from(keys: &[[u8; 3]]) -> Color32 {
    let best = keys
        .iter()
        .map(|&k| Hsva::from_srgb(k))
        // Vivid: saturated and not too dark.
        .max_by(|a, b| (a.s * a.v.sqrt()).total_cmp(&(b.s * b.v.sqrt())));
    match best {
        Some(h) if h.s * h.v.sqrt() >= 0.18 => {
            let h = Hsva { h: h.h, s: h.s.clamp(0.45, 0.85), v: h.v.clamp(0.82, 1.0), a: 1.0 };
            let [r, g, b] = h.to_srgb();
            Color32::from_rgb(r, g, b)
        }
        _ => NEUTRAL_ACCENT,
    }
}

/// A rounded card for floating controls.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_STROKE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .shadow(Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(90) })
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
        assert_eq!(accent_from(&[[0, 0, 0], [128, 128, 128], [255, 255, 255]]), NEUTRAL_ACCENT);
        // A dark navy palette still gives an accent bright enough for dark cards.
        let a = accent_from(&[[0, 0, 40], [10, 10, 90], [255, 255, 255]]);
        assert!(luminance(a) > 0.08, "{a:?}");
        assert!(a.b() > a.r() && a.b() > a.g(), "keeps the hue: {a:?}");
        // The most vivid key wins.
        let a = accent_from(&[[250, 250, 250], [230, 30, 20], [90, 80, 80]]);
        assert!(a.r() > 200 && a.g() < 120, "{a:?}");
    }

    #[test]
    fn text_on_accent_contrasts() {
        assert_eq!(on(Color32::from_rgb(250, 230, 80)), Color32::from_rgb(12, 12, 16));
        assert_eq!(on(Color32::from_rgb(40, 60, 200)), Color32::WHITE);
    }
}
