//! The frameless window: our own title strip, window buttons and resize
//! edges. macOS keeps its traffic lights over a hidden title bar; Windows
//! rounds the corners itself; elsewhere the window is transparent and we
//! paint the rounded corners.

use crate::theme;
use eframe::egui::{
    self, Align2, Color32, CursorIcon, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, Vec2, ViewportBuilder,
    ViewportCommand,
};

/// Whether the platform's own title bar and buttons are kept.
pub const NATIVE_BUTTONS: bool = cfg!(target_os = "macos");
/// Whether the window is transparent, so we paint its rounded corners.
pub const PAINT_CORNERS: bool = cfg!(all(unix, not(target_os = "macos")));

/// Above the canvas: room for macOS's traffic lights, else the usual gap.
pub fn top_gap() -> f32 {
    if NATIVE_BUTTONS { 30.0 } else { theme::GAP }
}

pub fn builder(viewport: ViewportBuilder) -> ViewportBuilder {
    let viewport = viewport.with_min_inner_size([720.0, 460.0]);
    if NATIVE_BUTTONS {
        viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false)
    } else {
        viewport.with_decorations(false).with_transparent(PAINT_CORNERS)
    }
}

/// Ask Windows 11 to round the frameless window's corners, as it does for
/// windows with a title bar. Older Windows ignores this.
#[cfg(windows)]
pub fn round_corners(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    let Ok(handle) = cc.window_handle() else { return };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let preference = DWMWCP_ROUND;
        // SAFETY: a valid window handle, and a pointer to a value of the size given.
        unsafe {
            DwmSetWindowAttribute(
                h.hwnd.get() as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const preference).cast(),
                size_of_val(&preference) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
pub fn round_corners(_cc: &eframe::CreationContext<'_>) {}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// Corner radius of the window's background.
pub fn window_radius(ctx: &egui::Context) -> f32 {
    if PAINT_CORNERS && !maximized(ctx) { theme::RADIUS as f32 } else { 0.0 }
}

pub fn toggle_maximized(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
}

/// Make a widget move the window when dragged, and maximise it when double-clicked.
pub fn drag_window(ctx: &egui::Context, response: &egui::Response) {
    if response.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        toggle_maximized(ctx);
    }
}

/// The strip along the top that moves the window, and the edges that resize
/// it. Call after the canvas, so these win over it.
pub fn frame_controls(ui: &mut egui::Ui, window: Rect, canvas: Rect) {
    let ctx = ui.ctx().clone();
    let strip = Rect::from_min_max(window.min, Pos2::new(window.max.x, canvas.top() + 10.0));
    let response = ui.interact(strip, Id::unique("title-strip"), Sense::CLICK | Sense::DRAG);
    drag_window(&ctx, &response);
    if NATIVE_BUTTONS || maximized(&ctx) {
        return;
    }
    let edge = 5.0;
    let corner = 14.0;
    let (l, r, t, b) = (window.left(), window.right(), window.top(), window.bottom());
    use ResizeDirection::*;
    let bands = [
        (North, Rect::from_min_max(Pos2::new(l + corner, t), Pos2::new(r - corner, t + edge))),
        (South, Rect::from_min_max(Pos2::new(l + corner, b - edge), Pos2::new(r - corner, b))),
        (West, Rect::from_min_max(Pos2::new(l, t + corner), Pos2::new(l + edge, b - corner))),
        (East, Rect::from_min_max(Pos2::new(r - edge, t + corner), Pos2::new(r, b - corner))),
        (NorthWest, Rect::from_min_size(Pos2::new(l, t), Vec2::splat(corner))),
        (NorthEast, Rect::from_min_size(Pos2::new(r - corner, t), Vec2::splat(corner))),
        (SouthWest, Rect::from_min_size(Pos2::new(l, b - corner), Vec2::splat(corner))),
        (SouthEast, Rect::from_min_size(Pos2::new(r - corner, b - corner), Vec2::splat(corner))),
    ];
    for (dir, rect) in bands {
        let id = Id::unique(("resize", format!("{dir:?}")));
        let response = ui.interact(rect, id, Sense::DRAG);
        let cursor = match dir {
            North | South => CursorIcon::ResizeVertical,
            East | West => CursorIcon::ResizeHorizontal,
            NorthWest | SouthEast => CursorIcon::ResizeNwSe,
            NorthEast | SouthWest => CursorIcon::ResizeNeSw,
        };
        if response.hovered() || response.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if response.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

/// Minimise, maximise and close, in a pill at the top right. Returns the
/// pill's bottom edge, or `None` where the platform draws its own buttons.
pub fn buttons(ctx: &egui::Context, canvas: Rect, inset: f32) -> Option<f32> {
    if NATIVE_BUTTONS {
        return None;
    }
    let max = maximized(ctx);
    let response = egui::Area::new(Id::unique("window-buttons"))
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(Pos2::new(canvas.right() - inset, canvas.top() + inset))
        .show(ctx, |ui| {
            theme::pill().inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if icon_button(ui, Icon::Minimise, "Minimise").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }
                    let (icon, hint) = if max { (Icon::Restore, "Restore") } else { (Icon::Maximise, "Maximise") };
                    if icon_button(ui, icon, hint).clicked() {
                        toggle_maximized(ctx);
                    }
                    if icon_button(ui, Icon::Close, "Close").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
            });
        })
        .response;
    Some(response.rect.bottom())
}

#[derive(Clone, Copy)]
enum Icon {
    Minimise,
    Maximise,
    Restore,
    Close,
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 24.0), Sense::click());
    let response = response.on_hover_text(hint);
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        let fill = match icon {
            Icon::Close => Color32::from_rgb(196, 43, 28),
            _ => theme::colours().wash.gamma_multiply(2.75),
        };
        painter.rect_filled(rect, 255, fill);
    }
    let colour = match (icon, hovered) {
        (Icon::Close, true) => Color32::WHITE,
        (_, true) => theme::colours().text_strong,
        _ => theme::colours().text,
    };
    let stroke = Stroke::new(1.2, colour);
    let c = rect.center();
    let s = 4.5;
    match icon {
        Icon::Minimise => {
            painter.line_segment([Pos2::new(c.x - s, c.y), Pos2::new(c.x + s, c.y)], stroke);
        }
        Icon::Maximise => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * s)), 1.5, stroke, egui::StrokeKind::Middle);
        }
        Icon::Restore => {
            let back = Rect::from_center_size(c + Vec2::new(1.5, -1.5), Vec2::splat(2.0 * s - 2.0));
            let front = Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(2.0 * s - 2.0));
            painter.rect_stroke(back, 1.5, stroke, egui::StrokeKind::Middle);
            painter.rect_filled(front, 1.5, theme::colours().card);
            painter.rect_stroke(front, 1.5, stroke, egui::StrokeKind::Middle);
        }
        Icon::Close => {
            painter.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(s, s)], stroke);
            painter.line_segment([c + Vec2::new(-s, s), c + Vec2::new(s, -s)], stroke);
        }
    }
    response
}
