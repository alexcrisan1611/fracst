//! Reproducible benchmark suite: `cargo run --release --example bench [-- --baseline] [--export]`
//!
//! Locations are generated with Newton's method from fixed starting points,
//! so no data files are needed. `--baseline` also renders each location with
//! BLA and interior detection disabled (plain perturbation with rebasing).
//! `--export` writes the locations to `bench-locations/*.kfr` so the same
//! views can be timed in Kalles Fraktaler 2.

use fracst_core::{Colouring, FloatExp, Progress, Renderer, Settings, View, kfr, newton};
use std::time::Instant;

struct Case {
    name: &'static str,
    view: View,
    /// Whether the plain-perturbation baseline finishes in reasonable time.
    baseline: bool,
}

/// Zoom to the minibrot found near (re, im) at `zoom`, scaled so it spans `fill` of the height.
fn minibrot(re: &str, im: &str, zoom: &str, fill: f64, maxiter: u64) -> View {
    let start = View::parse(re, im, zoom, maxiter).unwrap();
    let radius = start.pixel_spacing(1080) * FloatExp::from(540.0);
    let n = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).expect("minibrot");
    View { re: n.re, im: n.im, zoom: FloatExp::from(fill) / n.size, ..start }
}

fn near_i(digits: usize) -> (String, String) {
    (format!("0.{}2", "0".repeat(digits)), format!("1.{}3", "0".repeat(digits)))
}

fn main() {
    let baseline = std::env::args().any(|a| a == "--baseline");
    let export = std::env::args().any(|a| a == "--export");
    let seahorse = ("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139");
    let (r120, i120) = near_i(120);
    let (r200, i200) = near_i(200);
    let (r500, i500) = near_i(500);
    let cases = vec![
        Case { name: "overview", view: View { maxiter: 1000, ..View::default() }, baseline: true },
        Case {
            name: "seahorse minibrot ~3e8",
            view: minibrot(seahorse.0, seahorse.1, "1e8", 0.05, 100_000),
            baseline: true,
        },
        Case { name: "spiral ~1e238", view: minibrot(&r120, &i120, "1e120", 0.01, 200_000), baseline: true },
        Case { name: "spiral ~1e397", view: minibrot(&r200, &i200, "1e200", 0.01, 200_000), baseline: true },
        // Plain perturbation would iterate every interior pixel a million times.
        Case {
            name: "inside minibrot ~1e400, 1M iters",
            view: minibrot(&r200, &i200, "1e200", 2.0, 1_000_000),
            baseline: false,
        },
        Case { name: "spiral ~1e1000", view: minibrot(&r500, &i500, "1e500", 0.01, 1_000_000), baseline: true },
    ];
    let settings = Settings { width: 1920, height: 1080, ..Settings::default() };
    println!("{} threads, {}x{}", rayon::current_num_threads(), settings.width, settings.height);
    println!(
        "{:34} {:>11} {:>9} {:>9} {:>10} {:>9} {:>8}",
        "location", "zoom", "first", "again", "reference", "avg iter", "steps"
    );
    if export {
        std::fs::create_dir_all("bench-locations").unwrap();
    }
    for case in &cases {
        let mut view = case.view.clone();
        view.ensure_precision(settings.height);
        if export {
            let file: String = case.name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let path = format!("bench-locations/{file}.kfr");
            kfr::save(std::path::Path::new(&path), &view, &Colouring::default()).unwrap();
        }
        report(case.name, &view, &settings);
        if baseline && case.baseline {
            let plain = Settings { bla: false, interior_detection: false, ..settings.clone() };
            report("  (no BLA, no interior detection)", &view, &plain);
        }
    }
}

fn report(name: &str, view: &View, settings: &Settings) {
    let mut renderer = Renderer::new();
    let t = Instant::now();
    let first = renderer.render(view, settings, &Progress::default()).unwrap();
    let first_time = t.elapsed().as_secs_f64();
    // Second render reuses the reference orbit (as when recolouring or nudging the view).
    let t = Instant::now();
    let again = renderer.render(view, settings, &Progress::default()).unwrap();
    let again_time = t.elapsed().as_secs_f64();
    let st = &first.stats;
    let px = (first.width * first.height) as f64;
    println!(
        "{:34} {:>11} {:>8.3}s {:>8.3}s {:>9.3}s {:>9.0} {:>8.1}",
        name,
        format!("{:.2}", view.zoom),
        first_time,
        again_time,
        st.reference.as_secs_f64(),
        again.stats.iterations as f64 / px,
        again.stats.steps as f64 / px,
    );
}
