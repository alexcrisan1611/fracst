//! Compare rendered iteration counts against brute-force high-precision
//! iteration of individual pixels.

use fracst_core::{Fixed, FloatExp, Frame, INTERIOR, Progress, Renderer, Settings, View, newton};

/// Escape iteration of `c` by direct fixed-point iteration (None if it stays bounded).
fn direct(cr: &Fixed, ci: &Fixed, maxiter: u64, bailout2: f64) -> Option<u64> {
    let n = cr.limbs();
    let (mut x, mut y) = (Fixed::zero(n), Fixed::zero(n));
    for i in 0..=maxiter {
        let (fx, fy) = (x.to_f64(), y.to_f64());
        if fx * fx + fy * fy > bailout2 {
            return Some(i);
        }
        let x2 = x.sqr();
        let y2 = y.sqr();
        let xy = x.mul(&y);
        x = x2.sub(&y2).add(cr);
        y = xy.add(&xy).add(ci);
    }
    None
}

fn render(view: &View, s: &Settings) -> Frame {
    Renderer::new().render(view, s, &Progress::default()).unwrap()
}

/// Check every `stride`-th pixel; return the fraction whose iteration count differs.
fn compare(view: &View, s: &Settings, frame: &Frame, stride: usize) -> f64 {
    let spacing = view.pixel_spacing(s.height);
    let n = view.re.limbs() + 1;
    let (mut checked, mut bad) = (0, 0);
    for y in (0..s.height as usize).step_by(stride) {
        for x in (0..s.width as usize).step_by(stride) {
            let (px, py) = view.pixel_offset(x as f64 + 0.5, y as f64 + 0.5, s.width, s.height);
            let cr = view.re.with_limbs(n).add(&Fixed::from_floatexp(spacing * FloatExp::from(px), n));
            let ci = view.im.with_limbs(n).add(&Fixed::from_floatexp(spacing * FloatExp::from(py), n));
            let expect = direct(&cr, &ci, view.maxiter, s.bailout * s.bailout);
            let i = y * s.width as usize + x;
            // The renderer stores floor(smooth count), which is the escape iteration or one below it.
            let got = (frame.n[i] != INTERIOR).then(|| frame.n[i] as u64);
            let ok = match (expect, got) {
                (Some(e), Some(g)) => g == e || g + 1 == e,
                (None, None) => true,
                _ => false,
            };
            checked += 1;
            if !ok {
                bad += 1;
                eprintln!("pixel ({x},{y}): direct {expect:?} rendered {got:?}");
            }
        }
    }
    bad as f64 / checked as f64
}

fn settings(bla: bool) -> Settings {
    Settings { width: 96, height: 64, bla, ..Settings::default() }
}

fn check(view: &View, stride: usize, min_interior: f64) {
    for bla in [false, true] {
        let s = settings(bla);
        let frame = render(view, &s);
        let interior = frame.stats.interior as f64 / (s.width * s.height) as f64;
        assert!(interior >= min_interior, "expected interior pixels, got {:.1}%", 100.0 * interior);
        let bad = compare(view, &s, &frame, stride);
        assert!(
            bad <= 0.01,
            "zoom {} bla={bla}: {:.2}% of pixels wrong ({})",
            view.zoom,
            100.0 * bad,
            frame.stats.mode
        );
    }
}

/// A view centred slightly off the minibrot that Newton's method finds near
/// the starting location, with the minibrot spanning `fill` of the height.
fn minibrot_view(re: &str, im: &str, zoom: &str, fill: f64, maxiter: u64) -> View {
    minibrot_view_nudged(re, im, zoom, fill, maxiter, 0.37)
}

fn minibrot_view_nudged(re: &str, im: &str, zoom: &str, fill: f64, maxiter: u64, nudge_px: f64) -> View {
    let start = View::parse(re, im, zoom, maxiter).unwrap();
    let radius = start.pixel_spacing(64) * FloatExp::from(32.0);
    let nucleus = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).unwrap();
    let mut view = start.clone();
    view.zoom = FloatExp::from(fill) / nucleus.size;
    let n = nucleus.re.limbs() + 1;
    // Nudge off-centre so the reference orbit is not exactly periodic.
    let nudge = Fixed::from_floatexp(view.pixel_spacing(64) * FloatExp::from(nudge_px), n);
    view.re = nucleus.re.with_limbs(n).add(&nudge);
    view.im = nucleus.im.with_limbs(n).sub(&nudge);
    view
}

const SEAHORSE: (&str, &str) = ("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139");

fn near_i() -> (String, String) {
    (format!("0.{}2", "0".repeat(200)), format!("1.{}3", "0".repeat(200)))
}

#[test]
fn shallow_seahorse() {
    check(&View::parse(SEAHORSE.0, SEAHORSE.1, "1e6", 5000).unwrap(), 3, 0.0);
}

#[test]
fn misiurewicz_1e50() {
    check(
        &View::parse("0", "1.0000000000000000000000000000000000000000000000000000003", "1e50", 5000).unwrap(),
        3,
        0.0,
    );
}

#[test]
fn shallow_minibrot_f64() {
    check(&minibrot_view(SEAHORSE.0, SEAHORSE.1, "1e8", 3.0, 20000), 3, 0.05);
}

#[test]
fn deep_minibrot_1e400() {
    let (re, im) = near_i();
    let view = minibrot_view(&re, &im, "1e200", 3.0, 8000);
    assert!(view.zoom.log2() > 1300.0, "{}", view.zoom);
    check(&view, 4, 0.05);
}

#[test]
fn deep_minibrot_periodic_reference() {
    // Centred exactly on the nucleus: the reference stops after one period.
    let (re, im) = near_i();
    let view = minibrot_view_nudged(&re, &im, "1e200", 3.0, 30000, 0.0);
    let frame = render(&view, &settings(true));
    assert!(
        frame.stats.reference_truncated && frame.stats.reference_len < 1000,
        "reference length {}",
        frame.stats.reference_len
    );
    check(&view, 4, 0.05);
}

#[test]
fn deep_spiral_1e400() {
    let (re, im) = near_i();
    check(&minibrot_view(&re, &im, "1e200", 0.003, 20000), 3, 0.0);
}
