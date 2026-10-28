//! Views that exercise every path of the pixel loop, and a frame
//! comparison: an engine must reproduce the CPU's frames exactly. Used by
//! the tests (with the host build of the kernel) and by `fracst-server
//! --selftest` (with the GPU).

use fracst_core::perturb::PixelResult;
use fracst_core::{EnginePass, Fixed, FloatExp, Frame, PixelEngine, Progress, Renderer, Settings, View, newton};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Case {
    pub name: &'static str,
    pub view: View,
    pub settings: Settings,
}

fn minibrot(re: &str, im: &str, zoom: &str, fill: f64, maxiter: u64, nudge_px: f64) -> View {
    let start = View::parse(re, im, zoom, maxiter).unwrap();
    let radius = start.pixel_spacing(64) * FloatExp::from(32.0);
    let nucleus = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).unwrap();
    let mut view = start.clone();
    view.zoom = FloatExp::from(fill) / nucleus.size;
    let n = nucleus.re.limbs() + 1;
    let nudge = Fixed::from_floatexp(view.pixel_spacing(64) * FloatExp::from(nudge_px), n);
    view.re = nucleus.re.with_limbs(n).add(&nudge);
    view.im = nucleus.im.with_limbs(n).sub(&nudge);
    view
}

const SEAHORSE: (&str, &str) = ("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139");

fn near_i() -> (String, String) {
    (format!("0.{}2", "0".repeat(200)), format!("1.{}3", "0".repeat(200)))
}

/// Small frames covering `f64` and extended-range passes, BLA on and off,
/// both flavours of interior detection, distance estimation and automatic
/// iterations (which re-render only some pixels).
pub fn cases() -> Vec<Case> {
    let s = Settings { width: 160, height: 90, ..Settings::default() };
    let overview = View { maxiter: 2000, ..View::default() };
    let seahorse = minibrot(SEAHORSE.0, SEAHORSE.1, "1e8", 0.5, 20_000, 0.0);
    let (re, im) = near_i();
    let deep_minibrot = minibrot(&re, &im, "1e200", 3.0, 30_000, 0.0);
    let deep_off = minibrot(&re, &im, "1e200", 3.0, 30_000, 0.37);
    let deep_spiral = minibrot(&re, &im, "1e200", 0.003, 20_000, 0.37);
    vec![
        Case { name: "overview", view: overview.clone(), settings: s.clone() },
        Case {
            name: "overview, no interior detection",
            view: overview.clone(),
            settings: Settings { interior_detection: false, ..s.clone() },
        },
        Case { name: "overview, no BLA", view: overview.clone(), settings: Settings { bla: false, ..s.clone() } },
        Case {
            name: "overview, distance estimation",
            view: overview,
            settings: Settings { distance_estimate: true, ..s.clone() },
        },
        Case { name: "seahorse minibrot (periodic reference)", view: seahorse.clone(), settings: s.clone() },
        Case {
            name: "seahorse minibrot, off-centre (two-pass interior)",
            view: seahorse.clone(),
            settings: Settings { periodic_reference: false, ..s.clone() },
        },
        Case {
            name: "seahorse surroundings, auto iterations",
            view: View { zoom: seahorse.zoom * FloatExp::from(0.1), maxiter: 64, ..seahorse },
            settings: Settings { auto_iterations: true, ..s.clone() },
        },
        Case { name: "deep minibrot 1e400 (periodic reference)", view: deep_minibrot, settings: s.clone() },
        Case {
            name: "deep minibrot 1e400, off-centre",
            view: deep_off.clone(),
            settings: Settings { periodic_reference: false, ..s.clone() },
        },
        Case {
            name: "deep minibrot 1e400, distance estimation",
            view: deep_off,
            settings: Settings { distance_estimate: true, periodic_reference: false, ..s.clone() },
        },
        Case { name: "deep spiral 1e397", view: deep_spiral.clone(), settings: s.clone() },
        Case { name: "deep spiral 1e397, no BLA", view: deep_spiral, settings: Settings { bla: false, ..s } },
    ]
}

/// Pixels whose stored results differ between two frames, or `None` if the
/// frames differ in size or iteration limit.
pub fn differing_pixels(a: &Frame, b: &Frame) -> Option<usize> {
    if a.width != b.width || a.height != b.height || a.maxiter != b.maxiter || a.de.len() != b.de.len() {
        return None;
    }
    let de = |f: &Frame, i: usize| f.de.get(i).map(|d| d.to_bits());
    Some(
        (0..a.n.len())
            .filter(|&i| a.n[i] != b.n[i] || a.frac[i].to_bits() != b.frac[i].to_bits() || de(a, i) != de(b, i))
            .count(),
    )
}

pub struct Outcome {
    pub name: &'static str,
    /// `None`: the frames differ in size or limit.
    pub differing: Option<usize>,
    pub pixels: usize,
    pub steps_equal: bool,
    /// Passes the engine rendered (rather than leaving them to the CPU).
    pub engine_passes: usize,
    pub cpu: Duration,
    pub engine: Duration,
}

impl Outcome {
    pub fn identical(&self) -> bool {
        self.differing == Some(0) && self.steps_equal && self.engine_passes > 0
    }
}

/// Counts the passes an engine actually renders.
struct Counting {
    engine: Arc<dyn PixelEngine>,
    passes: AtomicUsize,
}

impl PixelEngine for Counting {
    fn name(&self) -> String {
        self.engine.name()
    }

    fn iterate(&self, pass: &EnginePass<'_>) -> Option<Vec<PixelResult>> {
        let results = self.engine.iterate(pass);
        if results.is_some() {
            self.passes.fetch_add(1, Ordering::Relaxed);
        }
        results
    }
}

/// Render `case` on the CPU and with `engine`, and compare.
pub fn run(case: &Case, engine: &Arc<dyn PixelEngine>) -> Outcome {
    let render = |engine: Option<Arc<dyn PixelEngine>>| {
        let mut r = Renderer::new();
        r.set_engine(engine);
        let t = Instant::now();
        let frame = r.render(&case.view, &case.settings, &Progress::default()).expect("render");
        (frame, t.elapsed())
    };
    let counting = Arc::new(Counting { engine: engine.clone(), passes: AtomicUsize::new(0) });
    let (cpu, cpu_time) = render(None);
    let (gpu, gpu_time) = render(Some(counting.clone()));
    Outcome {
        name: case.name,
        differing: differing_pixels(&cpu, &gpu),
        pixels: cpu.n.len(),
        steps_equal: cpu.stats.steps == gpu.stats.steps && cpu.stats.iterations == gpu.stats.iterations,
        engine_passes: counting.passes.load(Ordering::Relaxed),
        cpu: cpu_time,
        engine: gpu_time,
    }
}

/// The benchmark suite's locations (as in `fracst-core/examples/bench.rs`),
/// rendered at 1920x1080.
pub fn bench_cases() -> Vec<Case> {
    fn minibrot(re: &str, im: &str, zoom: &str, fill: f64, maxiter: u64) -> View {
        let start = View::parse(re, im, zoom, maxiter).unwrap();
        let radius = start.pixel_spacing(1080) * FloatExp::from(540.0);
        let n = newton::find_minibrot(&start.re, &start.im, radius, maxiter, &Progress::default()).expect("minibrot");
        View { re: n.re, im: n.im, zoom: FloatExp::from(fill) / n.size, ..start }
    }
    fn near_i(digits: usize) -> (String, String) {
        (format!("0.{}2", "0".repeat(digits)), format!("1.{}3", "0".repeat(digits)))
    }
    let s = Settings { width: 1920, height: 1080, ..Settings::default() };
    let (r120, i120) = near_i(120);
    let (r200, i200) = near_i(200);
    let (r500, i500) = near_i(500);
    let case = |name, mut view: View| {
        view.ensure_precision(s.height);
        Case { name, view, settings: s.clone() }
    };
    vec![
        case("overview", View { maxiter: 1000, ..View::default() }),
        case("seahorse minibrot ~3e8", minibrot(SEAHORSE.0, SEAHORSE.1, "1e8", 0.05, 100_000)),
        case("spiral ~1e238", minibrot(&r120, &i120, "1e120", 0.01, 200_000)),
        case("spiral ~1e397", minibrot(&r200, &i200, "1e200", 0.01, 200_000)),
        case("inside minibrot ~1e400, 1M iters", minibrot(&r200, &i200, "1e200", 2.0, 1_000_000)),
        case("spiral ~1e1000", minibrot(&r500, &i500, "1e500", 0.01, 1_000_000)),
    ]
}

/// Timings of one benchmark location: the first render (reference orbit and
/// BLA tables included) and the fastest of `repeat` renders that reuse them.
pub struct Timing {
    pub first: Duration,
    pub again: Duration,
    pub pixels: Duration,
}

pub fn time(case: &Case, engine: Option<Arc<dyn PixelEngine>>, repeat: usize) -> Timing {
    let mut r = Renderer::new();
    r.set_engine(engine);
    let t = Instant::now();
    let first = r.render(&case.view, &case.settings, &Progress::default()).expect("render");
    let first_time = t.elapsed();
    let (mut again, mut pixels) = (Duration::MAX, Duration::MAX);
    for _ in 0..repeat.max(1) {
        let t = Instant::now();
        let frame = r.render(&case.view, &case.settings, &Progress::default()).expect("render");
        again = again.min(t.elapsed());
        pixels = pixels.min(frame.stats.pixels);
    }
    let _ = first;
    Timing { first: first_time, again, pixels }
}
