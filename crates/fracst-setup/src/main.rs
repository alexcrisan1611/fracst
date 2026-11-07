//! fracst-setup: the Windows installer. It shows Fracst's own screens and
//! does the installing through the Inno Setup engine it carries
//! (packaging/windows/fracst.iss), run silently. See build.rs for how the
//! engine gets in.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod look;

#[cfg(windows)]
fn main() -> eframe::Result {
    use eframe::egui;
    let args = install::Args::parse();
    let title = if install::find_existing().is_some() { "Update Fracst" } else { "Install Fracst" };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([640.0, 500.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_decorations(false);
    if let Ok((rgba, width, height)) = fracst_core::image::decode_rgba_png(include_bytes!("../../../assets/icon.png")) {
        viewport = viewport.with_icon(egui::IconData { rgba, width, height });
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "fracst-setup",
        options,
        Box::new(move |cc| {
            round_corners(cc);
            Ok(Box::new(app::App::new(cc, args)))
        }),
    )
}

/// Ask Windows 11 to round the frameless window's corners. Older Windows
/// ignores this.
#[cfg(windows)]
fn round_corners(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    let Ok(handle) = cc.window_handle() else { return };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let preference = DWMWCP_ROUND;
        // A quiet border instead of the system's accent-coloured one (a
        // COLORREF: 0x00BBGGRR).
        let border: u32 = 0x0038_3030;
        // SAFETY: a valid window handle, and pointers to values of the sizes given.
        unsafe {
            DwmSetWindowAttribute(
                h.hwnd.get() as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const preference).cast(),
                size_of_val(&preference) as u32,
            );
            DwmSetWindowAttribute(
                h.hwnd.get() as _,
                DWMWA_BORDER_COLOR as u32,
                (&raw const border).cast(),
                size_of_val(&border) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fracst-setup is the Windows installer; it does nothing elsewhere.");
    std::process::exit(1);
}
