//! The SIMD pixel loop must give exactly the scalar loop's results.

use fracst_core::{FloatExp, Frame, Progress, Renderer, Settings, View, newton};

fn render(view: &View, s: &Settings) -> Frame {
    Renderer::new().render(view, s, &Progress::default()).unwrap()
}

fn assert_same(name: &str, view: &View, s: &Settings) {
    let a = render(view, &Settings { simd: false, ..s.clone() });
    let b = render(view, &Settings { simd: true, ..s.clone() });
    assert_eq!(a.maxiter, b.maxiter, "{name}: limits differ");
    assert!(a.n == b.n, "{name}: iteration counts differ");
    assert!(a.frac.iter().zip(&b.frac).all(|(x, y)| x.to_bits() == y.to_bits()), "{name}: smooth parts differ");
    assert_eq!(a.stats.steps, b.stats.steps, "{name}: step counts differ");
}

fn seahorse_minibrot(fill: f64, maxiter: u64) -> View {
    let start =
        View::parse("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139", "1e8", maxiter)
            .unwrap();
    let radius = start.pixel_spacing(90) * FloatExp::from(45.0);
    let n = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).unwrap();
    View { re: n.re, im: n.im, zoom: FloatExp::from(fill) / n.size, ..start }
}

#[test]
fn simd_matches_scalar() {
    let small = Settings { width: 161, height: 90, ..Settings::default() };
    let overview = View { maxiter: 2000, ..View::default() };
    assert_same("overview", &overview, &small);
    assert_same("overview, no interior detection", &overview, &Settings { interior_detection: false, ..small.clone() });
    assert_same("overview, no BLA", &overview, &Settings { bla: false, ..small.clone() });
    // Periodic reference: period-end interior detection.
    assert_same("seahorse minibrot", &seahorse_minibrot(0.5, 20_000), &small);
    // Off the nucleus: per-iteration interior detection with the two-pass restart.
    let mut off = seahorse_minibrot(0.5, 20_000);
    off.translate_pixels(30.0, 7.0, small.height);
    assert_same("off-centre minibrot", &off, &Settings { periodic_reference: false, ..small.clone() });
    // Automatic iterations re-render only some pixels.
    assert_same(
        "auto iterations",
        &View { maxiter: 64, ..seahorse_minibrot(0.05, 20_000) },
        &Settings { auto_iterations: true, ..small.clone() },
    );
}
