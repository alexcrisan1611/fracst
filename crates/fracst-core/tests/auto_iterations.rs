//! Automatic iteration limits: they must resolve minibrots, stay bounded,
//! come back down, and produce the same pixels as a fixed limit.

use fracst_core::{Fixed, FloatExp, Frame, Progress, Renderer, Settings, View, auto_floor, newton};

fn auto_settings() -> Settings {
    Settings { width: 160, height: 90, auto_iterations: true, ..Settings::default() }
}

fn render(view: &View, s: &Settings) -> Frame {
    Renderer::new().render(view, s, &Progress::default()).unwrap()
}

fn report(name: &str, f: &Frame) {
    let st = &f.stats;
    eprintln!(
        "{name}: limit {} rounds {} suggested {} period {:?} escaped {}..{} interior {} undecided {}",
        f.maxiter,
        st.rounds,
        st.suggested_maxiter,
        st.nearby_period,
        st.min_escaped,
        st.max_escaped,
        st.interior,
        st.undecided
    );
}

/// Escape iteration by brute-force fixed-point iteration.
fn direct(cr: &Fixed, ci: &Fixed, maxiter: u64, bailout2: f64) -> Option<u64> {
    let n = cr.limbs();
    let (mut x, mut y) = (Fixed::zero(n), Fixed::zero(n));
    for i in 0..=maxiter {
        let (fx, fy) = (x.to_f64(), y.to_f64());
        if fx * fx + fy * fy > bailout2 {
            return Some(i);
        }
        let (x2, y2, xy) = (x.sqr(), y.sqr(), x.mul(&y));
        x = x2.sub(&y2).add(cr);
        y = xy.add(&xy).add(ci);
    }
    None
}

/// Fraction of sampled pixels whose escape iteration disagrees with brute force at the frame's limit.
fn wrong_fraction(view: &View, s: &Settings, f: &Frame, stride: usize) -> f64 {
    let spacing = view.pixel_spacing(s.height);
    let n = view.re.limbs() + 1;
    let (mut checked, mut bad) = (0, 0);
    for y in (0..s.height as usize).step_by(stride) {
        for x in (0..s.width as usize).step_by(stride) {
            let (px, py) = view.pixel_offset(x as f64 + 0.5, y as f64 + 0.5, s.width, s.height);
            let cr = view.re.with_limbs(n).add(&Fixed::from_floatexp(spacing * FloatExp::from(px), n));
            let ci = view.im.with_limbs(n).add(&Fixed::from_floatexp(spacing * FloatExp::from(py), n));
            let expect = direct(&cr, &ci, f.maxiter, s.bailout * s.bailout);
            let i = y * s.width as usize + x;
            let got = f.escaped(i).then(|| f.n[i] as u64);
            checked += 1;
            if !matches!((expect, got), (Some(e), Some(g)) if g == e || g + 1 == e)
                && !(expect.is_none() && got.is_none())
            {
                bad += 1;
                eprintln!("  ({x},{y}): direct {expect:?} rendered {got:?} (code {})", f.n[i]);
            }
        }
    }
    bad as f64 / checked as f64
}

#[test]
fn overview_does_not_run_away() {
    let view = View { maxiter: 100, ..View::default() };
    let f = render(&view, &auto_settings());
    report("overview", &f);
    assert!(f.maxiter >= auto_floor(view.zoom));
    assert!(f.maxiter <= 1 << 18, "limit ran away to {}", f.maxiter);
    assert!(f.stats.suggested_maxiter <= 1 << 18);
}

#[test]
fn excessive_hint_comes_back_down() {
    // The view from the bug report: a 16M limit at zoom 8.4e6.
    let view = View {
        maxiter: 16_384_000,
        ..View::parse("-0.36137208276443203486560701", "0.63768597663877987634901917", "8.388604E6", 0).unwrap()
    };
    let f = render(&view, &auto_settings());
    report("16M hint", &f);
    assert!(f.stats.suggested_maxiter < 200_000, "suggested {}", f.stats.suggested_maxiter);
    // Starting from the suggestion reproduces the picture with a far lower limit.
    let again = render(&View { maxiter: f.stats.suggested_maxiter, ..view }, &auto_settings());
    report("16M hint, next frame", &again);
    assert!(again.maxiter < 200_000);
    let differ = (0..f.n.len()).filter(|&i| f.escaped(i) != again.escaped(i)).count();
    assert!(differ as f64 / f.n.len() as f64 <= 0.002, "{differ} pixels changed class");
}

#[test]
fn nearby_minibrot_raises_low_hint() {
    let (re, im) = (format!("0.{}2", "0".repeat(200)), format!("1.{}3", "0".repeat(200)));
    let start = View::parse(&re, &im, "1e200", 100_000).unwrap();
    let radius = start.pixel_spacing(64) * FloatExp::from(32.0);
    let nucleus = newton::find_minibrot(&start.re, &start.im, radius, 100_000, &Progress::default()).unwrap();
    // The minibrot fills a third of the view; start from a hopelessly low hint.
    let view = View { re: nucleus.re, im: nucleus.im, zoom: FloatExp::from(0.3) / nucleus.size, maxiter: 50, ..start };
    let s = auto_settings();
    let f = render(&view, &s);
    report("deep minibrot", &f);
    assert_eq!(f.stats.nearby_period, Some(nucleus.period));
    assert!(f.maxiter >= nucleus.period * 8);
    assert!(f.stats.interior > 0, "the minibrot should be resolved");
    assert!((f.stats.undecided as f64) < 0.01 * f.n.len() as f64, "{} undecided pixels", f.stats.undecided);
    assert!(wrong_fraction(&view, &s, &f, 7) <= 0.01);
}

#[test]
fn correction_rounds_match_brute_force() {
    // A minibrot off to the side of a shallow view, with a hint far too low:
    // whatever mix of prediction and correction rounds this takes, the
    // result must match brute force at the limit finally used.
    let seahorse =
        View::parse("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139", "1e8", 0).unwrap();
    let radius = seahorse.pixel_spacing(64) * FloatExp::from(32.0);
    let nucleus = newton::find_minibrot(&seahorse.re, &seahorse.im, radius, 100_000, &Progress::default()).unwrap();
    let mut view =
        View { re: nucleus.re, im: nucleus.im, zoom: FloatExp::from(0.05) / nucleus.size, maxiter: 64, ..seahorse };
    let s = auto_settings();
    // Shift the minibrot towards the left edge of the view.
    view.translate_pixels(55.0, 10.0, s.height);
    view.ensure_precision(s.height);
    let f = render(&view, &s);
    report("off-centre minibrot", &f);
    assert!(f.maxiter > 64);
    assert!(wrong_fraction(&view, &s, &f, 3) <= 0.01);
}
