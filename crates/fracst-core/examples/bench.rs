//! Reproducible benchmark suite:
//! `cargo run --release --example bench [-- --baseline] [--export] [--repeat N] [--only TEXT] [--csv]`
//!
//! Locations are generated with Newton's method from fixed starting points,
//! so no data files are needed. `--baseline` also renders each location with
//! BLA and interior detection disabled (plain perturbation with rebasing).
//! `--export` writes the locations to `bench-locations/*.kfr` so the same
//! views can be rendered again with other settings or later versions.
//! `--repeat N` re-renders each location N times after the first render
//! (reusing the reference) and reports the fastest; `--only TEXT` keeps
//! locations whose name contains TEXT; `--csv` prints machine-readable rows.

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
    let args: Vec<String> = std::env::args().collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let baseline = flag("--baseline");
    let export = flag("--export");
    let csv = flag("--csv");
    let repeat: usize = value("--repeat").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let only = value("--only");
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
    let cases: Vec<Case> =
        cases.into_iter().filter(|c| only.as_ref().is_none_or(|o| c.name.contains(o.as_str()))).collect();
    let settings = Settings { width: 1920, height: 1080, ..Settings::default() };
    if csv {
        println!("location,first_s,again_s,reference_s,avg_iterations,avg_steps");
    } else {
        println!("{} threads, {}x{}", rayon::current_num_threads(), settings.width, settings.height);
        println!(
            "{:34} {:>11} {:>9} {:>9} {:>10} {:>9} {:>8}",
            "location", "zoom", "first", "again", "reference", "avg iter", "steps"
        );
    }
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
        report(case.name, &view, &settings, repeat, csv);
        if baseline && case.baseline {
            let plain = Settings { bla: false, interior_detection: false, ..settings.clone() };
            report(&format!("{} (plain)", case.name), &view, &plain, repeat, csv);
        }
    }
}

fn report(name: &str, view: &View, settings: &Settings, repeat: usize, csv: bool) {
    let mut renderer = Renderer::new();
    let t = Instant::now();
    let first = renderer.render(view, settings, &Progress::default()).unwrap();
    let first_time = t.elapsed().as_secs_f64();
    // Later renders reuse the reference orbit (as when recolouring or nudging the view).
    let mut again_time = f64::INFINITY;
    let mut again = None;
    for _ in 0..repeat {
        let t = Instant::now();
        let frame = renderer.render(view, settings, &Progress::default()).unwrap();
        again_time = again_time.min(t.elapsed().as_secs_f64());
        again = Some(frame);
    }
    let again = again.unwrap();
    let st = &first.stats;
    let px = (first.width * first.height) as f64;
    if csv {
        println!(
            "{name},{first_time:.4},{again_time:.4},{:.4},{:.1},{:.2}",
            st.reference.as_secs_f64(),
            again.stats.iterations as f64 / px,
            again.stats.steps as f64 / px,
        );
        return;
    }
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
