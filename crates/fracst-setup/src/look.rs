//! The installer's look: Fracst's dark scheme and Inter, a quiet background
//! (the icon's polar grid, fading out behind the text), and
//! the few widgets the screens need, drawn here rather than by egui's
//! defaults.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Id, Mesh, Pos2,
    Rect, Response, Role, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, epaint::Shadow,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(11, 11, 14);
pub const TEXT: Color32 = Color32::from_rgb(238, 238, 243);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(156, 156, 168);
pub const FIELD: Color32 = Color32::from_rgb(22, 22, 28);
pub const LINE: Color32 = Color32::from_rgb(46, 46, 56);
pub const LINE_HOVER: Color32 = Color32::from_rgb(78, 78, 92);
/// The icon's violet (its tile runs from #A784F6 to #9330F2).
pub const ACCENT: Color32 = Color32::from_rgb(140, 72, 238);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(156, 96, 244);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(120, 56, 214);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(196, 170, 252);
pub const LILAC: Color32 = Color32::from_rgb(167, 132, 246);

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"))),
    );
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.insert(
        FontFamily::Name("strong".into()),
        std::iter::once("inter-semibold".to_string()).chain(fallbacks).collect(),
    );
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.selection.bg_fill = ACCENT;
        style.interaction.selectable_labels = false;
    });
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("strong".into()))
}

// ---- Background -------------------------------------------------------------

/// The window's background: near-black, and the icon's polar grid (rings,
/// spokes and diamond anchors) centred off the lower right, fading out
/// before it reaches the text on the left.
pub fn background(ui: &Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0, BG);

    let centre = rect.right_bottom() + Vec2::new(-110.0, -120.0);
    let reach = 520.0;
    let fade = |r: f32| (1.0 - r / reach).clamp(0.0, 1.0);
    glow(painter, centre, 340.0, Color32::from_rgba_unmultiplied(120, 60, 220, 30));

    let ring_step = 52.0;
    let rings = (reach / ring_step) as usize;
    for k in 1..=rings {
        let r = k as f32 * ring_step;
        painter.circle_stroke(centre, r, Stroke::new(1.0, LILAC.gamma_multiply(0.16 * fade(r))));
    }
    // Spokes, drawn ring to ring so they fade with distance too.
    let spokes = 24;
    for j in 0..spokes {
        let dir = Vec2::angled(j as f32 / spokes as f32 * std::f32::consts::TAU);
        for k in 0..rings {
            let (r0, r1) = (k as f32 * ring_step + 6.0, (k + 1) as f32 * ring_step);
            let stroke = Stroke::new(1.0, LILAC.gamma_multiply(0.10 * fade(r1)));
            painter.line_segment([centre + dir * r0, centre + dir * r1], stroke);
        }
    }
    // Anchors on some of the crossings, as on the icon's tile.
    for k in 1..=rings {
        for j in 0..spokes {
            if (j * 5 + k * 3) % 7 != 0 {
                continue;
            }
            let r = k as f32 * ring_step;
            let p = centre + Vec2::angled(j as f32 / spokes as f32 * std::f32::consts::TAU) * r;
            diamond(painter, p, 3.2, LILAC.gamma_multiply(0.7 * fade(r)));
        }
    }
    diamond(painter, centre, 4.0, LILAC.gamma_multiply(0.8));
}

fn diamond(painter: &egui::Painter, p: Pos2, r: f32, colour: Color32) {
    painter.add(Shape::convex_polygon(
        vec![p + Vec2::new(0.0, -r), p + Vec2::new(r, 0.0), p + Vec2::new(0.0, r), p + Vec2::new(-r, 0.0)],
        colour,
        Stroke::NONE,
    ));
}

/// A radial glow: `colour` at the centre, fading to nothing at `radius`.
pub fn glow(painter: &egui::Painter, centre: Pos2, radius: f32, colour: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, colour);
    let n = 64;
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(centre + Vec2::angled(a) * radius, Color32::TRANSPARENT);
    }
    for k in 1..=n as u32 {
        mesh.add_triangle(0, k, k + 1);
    }
    painter.add(Shape::mesh(mesh));
}

// ---- Widgets ----------------------------------------------------------------

fn focus_ring(ui: &Ui, rect: Rect, radius: f32, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(rect.expand(3.0), radius + 3.0, Stroke::new(2.0, LILAC), StrokeKind::Middle);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Quiet,
}

/// A button. Its width fits its label, at least `min_width`; `right` and
/// `y` place its right edge and top.
pub fn button(ui: &mut Ui, right: f32, y: f32, label: &str, kind: Kind, min_width: f32) -> Response {
    let font = strong(14.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let w = (galley.size().x + 40.0).max(min_width);
    let rect = Rect::from_min_size(Pos2::new(right - w, y), Vec2::new(w, 40.0));
    let response = ui.interact(rect, Id::unique(("button", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let (fill, text, stroke) = match kind {
        Kind::Primary if response.is_pointer_button_down_on() => (ACCENT_PRESSED, Color32::WHITE, Stroke::NONE),
        Kind::Primary if response.hovered() => (ACCENT_HOVER, Color32::WHITE, Stroke::NONE),
        Kind::Primary => (ACCENT, Color32::WHITE, Stroke::NONE),
        Kind::Quiet if response.hovered() => (Color32::from_white_alpha(14), TEXT, Stroke::new(1.0, LINE_HOVER)),
        Kind::Quiet => (Color32::from_white_alpha(6), TEXT, Stroke::new(1.0, LINE)),
    };
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let radius = 10.0;
    let painter = ui.painter();
    if kind == Kind::Primary {
        // A soft glow of the button's own colour under it.
        let shadow = Shadow { offset: [0, 6], blur: 22, spread: 0, color: ACCENT.gamma_multiply(0.35) };
        painter.add(shadow.as_shape(rect, radius));
    }
    painter.rect(rect, radius, fill, stroke, StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, text);
    focus_ring(ui, rect, radius, &response);
    response
}

/// The install folder: a field showing the path, with Change inside it.
pub fn folder_field(ui: &mut Ui, rect: Rect, path: &str) -> Response {
    let response = ui.interact(rect, Id::unique("folder"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Install folder: {path}. Change")));
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    let radius = 10.0;
    painter.rect(rect, radius, FIELD, Stroke::new(1.0, if hovered { LINE_HOVER } else { LINE }), StrokeKind::Inside);

    let change = painter.layout_no_wrap("Change".into(), strong(13.0), ACCENT_TEXT);
    let change_pos = Pos2::new(rect.right() - 16.0 - change.size().x, rect.center().y - change.size().y / 2.0);
    let room = change_pos.x - 14.0 - (rect.left() + 14.0);
    let text = elide_middle(painter, path, regular(13.5), room);
    painter.text(Pos2::new(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, text, regular(13.5), TEXT);
    let colour = if hovered { Color32::WHITE } else { ACCENT_TEXT };
    painter.galley(change_pos, change, colour);
    focus_ring(ui, rect, radius, &response);
    response
}

/// `text`, shortened in the middle with … until it fits `width`.
fn elide_middle(painter: &egui::Painter, text: &str, font: FontId, width: f32) -> String {
    let fits = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), TEXT).size().x <= width;
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 4 {
        keep -= 1;
        let head = keep / 3;
        let tail = keep - head;
        let s: String = chars[..head].iter().chain(['…'].iter()).chain(chars[chars.len() - tail..].iter()).collect();
        if fits(&s) {
            return s;
        }
    }
    text.to_owned()
}

/// A row with a switch and its label; the whole row toggles `on`.
pub fn switch_row(ui: &mut Ui, rect: Rect, label: &str, on: &mut bool) -> Response {
    let mut response = ui.interact(rect, Id::unique(("switch", label)), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, *on, label));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.12);
    let track = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 11.0), Vec2::new(38.0, 22.0));
    let painter = ui.painter();
    let off_fill = if response.hovered() { Color32::from_rgb(54, 54, 66) } else { Color32::from_rgb(40, 40, 50) };
    let fill = lerp_colour(off_fill, ACCENT, t);
    painter.rect_filled(track, CornerRadius::same(11), fill);
    let knob_x = egui::lerp(track.left() + 11.0..=track.right() - 11.0, t);
    painter.circle_filled(
        Pos2::new(knob_x, track.center().y),
        8.0,
        lerp_colour(Color32::from_gray(190), Color32::WHITE, t),
    );
    painter.text(Pos2::new(track.right() + 14.0, rect.center().y), Align2::LEFT_CENTER, label, regular(14.0), TEXT);
    focus_ring(ui, track, 11.0, &response);
    response
}

fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A thin progress bar with a highlight that travels along the filled part.
pub fn progress(ui: &Ui, rect: Rect, fraction: f32, time: f64) {
    let painter = ui.painter();
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, Color32::from_rgb(32, 32, 40));
    let filled = Rect::from_min_size(rect.min, Vec2::new((rect.width() * fraction).max(rect.height()), rect.height()));
    painter.rect_filled(filled, r, ACCENT);
    let phase = (time * 0.9).fract() as f32;
    let x = egui::lerp(filled.left() - 60.0..=filled.right() + 60.0, phase);
    let band =
        Rect::from_center_size(Pos2::new(x, filled.center().y), Vec2::new(60.0, rect.height())).intersect(filled);
    if band.width() > 0.0 {
        painter.rect_filled(band, r, Color32::from_white_alpha(40));
    }
}

/// The small close button at the top right of the window.
pub fn close_button(ui: &mut Ui, window: Rect) -> Response {
    let rect = Rect::from_min_size(Pos2::new(window.right() - 44.0, window.top() + 10.0), Vec2::new(32.0, 28.0));
    let response = ui.interact(rect, Id::unique("close"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Close"));
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 8.0, Color32::from_white_alpha(16));
    }
    let c = rect.center();
    let s = 5.0;
    let stroke = Stroke::new(1.4, if response.hovered() { TEXT } else { TEXT_WEAK });
    painter.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(s, s)], stroke);
    painter.line_segment([c + Vec2::new(-s, s), c + Vec2::new(s, -s)], stroke);
    focus_ring(ui, rect, 8.0, &response);
    response
}

/// A text link; returns its response.
pub fn link(ui: &mut Ui, pos: Pos2, label: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), regular(13.5), ACCENT_TEXT);
    let rect = Rect::from_min_size(pos, galley.size());
    let response = ui.interact(rect.expand(2.0), Id::unique(("link", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Link, true, label));
    let colour = if response.hovered() { Color32::WHITE } else { ACCENT_TEXT };
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        ui.painter().line_segment(
            [rect.left_bottom() + Vec2::new(0.0, 1.0), rect.right_bottom() + Vec2::new(0.0, 1.0)],
            Stroke::new(1.0, colour),
        );
    }
    ui.painter().galley(pos, galley, colour);
    focus_ring(ui, rect.expand(2.0), 4.0, &response);
    response
}
