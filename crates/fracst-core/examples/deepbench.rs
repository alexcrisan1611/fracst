//! Single-thread microbenchmark of the extended-range (deep) phase:
//! `cargo run --release -p fracst-core --example deepbench`
//!
//! Renders small frames of a minibrot near c = i at ~1e400 on one thread and
//! times single pixels of the same view, reporting nanoseconds per loop step.

use fracst_core::bla::BlaTable;
use fracst_core::num::{CExp, Complex, Cplx};
use fracst_core::perturb::{self, Extras, Phase};
use fracst_core::reference::Reference;
use fracst_core::{FloatExp, Progress, Renderer, Settings, View, newton};
use std::time::Instant;

fn minibrot_view(fill: f64, maxiter: u64) -> View {
    let (re, im) = (format!("0.{}2", "0".repeat(200)), format!("1.{}3", "0".repeat(200)));
    let start = View::parse(&re, &im, "1e200", maxiter).unwrap();
    let radius = start.pixel_spacing(108) * FloatExp::from(54.0);
    let n = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).unwrap();
    View { re: n.re, im: n.im, zoom: FloatExp::from(fill) / n.size, ..start }
}

fn frame(name: &str, view: &View, reps: usize, interior_detection: bool) {
    let s = Settings { width: 192, height: 108, interior_detection, ..Settings::default() };
    let mut view = view.clone();
    view.ensure_precision(s.height);
    let mut r = Renderer::new();
    r.render(&view, &s, &Progress::default()).unwrap();
    let mut best = f64::INFINITY;
    let mut stats = None;
    for _ in 0..reps {
        let t = Instant::now();
        let f = r.render(&view, &s, &Progress::default()).unwrap();
        best = best.min(t.elapsed().as_secs_f64());
        stats = Some(f.stats);
    }
    let st = stats.unwrap();
    println!(
        "frame {name:22} {:8.1} ms  {:6.1} ns/step  steps {} (deep {:.0}%)",
        best * 1e3,
        best * 1e9 / st.steps as f64,
        st.steps,
        100.0 * st.deep_steps as f64 / st.steps as f64
    );
}

fn pixels(view: &View, maxiter: u64, reps: usize) {
    let h = 108u32;
    let mut view = view.clone();
    view.ensure_precision(h);
    let spacing = view.pixel_spacing(h);
    let p = Progress::default();
    let limbs = View::limbs_for(view.zoom, h) + 1;
    let r = Reference::compute(
        &view.re.with_limbs(limbs),
        &view.im.with_limbs(limbs),
        maxiter,
        spacing,
        FloatExp::ZERO,
        &p,
    )
    .unwrap();
    let cmax = spacing * FloatExp::from(111.0);
    let fe = BlaTable::<Complex<FloatExp>>::build(r.last_index(), |m| r.z_fe(m), cmax, 1.0 / (1u64 << 40) as f64);
    let deep_t: BlaTable<CExp> = fe.cast();
    let t64: BlaTable<Complex<f64>> = fe.cast();
    for (label, px, py) in [("interior (centre)", 0.5, 0.5), ("exterior (edge)", 90.5, 50.5)] {
        let dc = Complex::new(spacing * FloatExp::from(px), spacing * FloatExp::from(py));
        let deep = Phase {
            bla: &deep_t,
            dc: CExp::from_cfe(dc),
            spacing,
            bailout2: FloatExp::from(1e8),
            switch2: perturb::switch_norm(),
        };
        let shallow = Phase {
            bla: &t64,
            dc: Complex::new(perturb::flush_f64(dc.re), perturb::flush_f64(dc.im)),
            spacing: 0.0,
            bailout2: 1e8,
            switch2: perturb::switch_norm(),
        };
        let extras = Extras { distance_estimate: false, interior_detection: false };
        let mut best = f64::INFINITY;
        let mut res = perturb::iterate_deep(&r, &deep, &shallow, maxiter, extras);
        for _ in 0..reps {
            let t = Instant::now();
            res = perturb::iterate_deep(&r, &deep, &shallow, maxiter, extras);
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!(
            "pixel {label:22} {:8.1} us  {:6.1} ns/step  steps {} (deep {}) n {} escaped {}",
            best * 1e6,
            best * 1e9 / res.steps as f64,
            res.steps,
            res.deep_steps,
            res.n,
            res.escaped
        );
    }
}

fn main() {
    // One render thread: per-step costs without SMT or memory contention effects.
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("RAYON_NUM_THREADS", "1") };
    let inside = minibrot_view(2.0, 1_000_000);
    let spiral = minibrot_view(0.01, 200_000);
    pixels(&inside, 1_000_000, 20);
    frame("inside minibrot 1M", &View { maxiter: 1_000_000, ..inside.clone() }, 3, true);
    frame("inside 100k, no interior", &View { maxiter: 100_000, ..inside }, 3, false);
    frame("spiral 200k", &View { maxiter: 200_000, ..spiral }, 3, true);
}
