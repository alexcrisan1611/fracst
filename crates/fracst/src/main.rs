//! fracst: an interactive deep-zoom Mandelbrot explorer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod remote_config;
mod update;
mod worker;

fn main() -> eframe::Result {
    let mut viewport = eframe::egui::ViewportBuilder::default().with_title("fracst").with_inner_size([1400.0, 860.0]);
    if let Ok((rgba, width, height)) = fracst_core::image::decode_rgba_png(include_bytes!("../../../assets/icon.png")) {
        viewport = viewport.with_icon(eframe::egui::IconData { rgba, width, height });
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let file = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    eframe::run_native(
        "fracst",
        options,
        Box::new(move |cc| {
            let mut app = app::App::new(cc);
            if let Some(path) = &file {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}
