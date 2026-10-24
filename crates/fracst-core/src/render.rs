//! Frame rendering: reference orbit, BLA table and the parallel pixel loop,
//! plus automatic iteration limits.
//!
//! ## Automatic iterations
//!
//! A fixed iteration limit is either too low near minibrots (their
//! surroundings render as solid interior) or wastefully high elsewhere. With
//! [`Settings::auto_iterations`] the limit is chosen per frame:
//!
//! 1. **Prediction.** While computing the reference orbit, a ball test finds
//!    the lowest period `P` of a minibrot within a few view radii. Pixels
//!    around it escape at multiples of `P`, so the limit starts at least at
//!    `PERIOD_FACTOR * P`. Without a nearby minibrot the start is the
//!    caller's hint, never below a floor that grows gently with zoom.
//! 2. **Correction.** After the pass, pixels that hit the limit without
//!    escaping or being detected as interior are *undecided*. If many
//!    pixels escaped just below the limit, or undecided pixels are common
//!    and not merely a thin rim around detected interior, the limit is
//!    raised and only the undecided pixels are rendered again.
//!    A round that fails to resolve most of the remaining undecided pixels
//!    ends the escalation: those are interior pixels converging slowly.
//! 3. **Suggestion.** The frame reports the limit the next frame should
//!    start from: twice the escape time of all but the slowest 0.01% of
//!    pixels, so isolated stragglers next to the boundary cannot ratchet the
//!    limit upwards, and it falls again when a view needs less.
//!
//! ## Periodic references
//!
//! With [`Settings::periodic_reference`], a minibrot found near the view is
//! located precisely with Newton's method and used as the reference. Its
//! orbit stops after one period instead of running to the iteration limit,
//! and pixels rebase at exactly the period, which makes interior detection
//! reliable (see `perturb`).

use crate::Progress;
use crate::bla::BlaTable;
use crate::floatexp::FloatExp;
use crate::num::{CExp, Complex, Cplx};
use crate::perturb::{self, Extras, Phase, PixelResult};
use crate::reference::Reference;
use crate::view::View;
use crate::{fixed::Fixed, newton};
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Marks pixels detected as interior (attracted to a cycle).
pub const INTERIOR: u32 = u32::MAX;
/// Marks pixels that reached the iteration limit without escaping or being
/// detected as interior. Coloured like interior.
pub const UNDECIDED: u32 = u32::MAX - 1;

/// Automatic limits start at least at this multiple of a nearby minibrot's period.
const PERIOD_FACTOR: u64 = 8;
/// Minibrots are looked for within this many view radii of the centre.
const PROBE_RADII: f64 = 4.0;
/// Each correction round multiplies the limit by this.
const RAISE_FACTOR: u64 = 4;
const MAX_ROUNDS: u32 = 5;
/// Fraction of pixels allowed to escape in the top half of the limit, or to
/// stay undecided, before the limit is raised (but at least `MIN_PIXELS`).
const TAIL_FRACTION: f64 = 1e-4;
const MIN_PIXELS: f64 = 16.0;

/// Smallest automatic iteration limit at a given zoom.
pub fn auto_floor(zoom: FloatExp) -> u64 {
    (1024.0 + 16.0 * zoom.log2().max(0.0)) as u64
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    /// Escape radius. Large values give smoother colouring.
    pub bailout: f64,
    pub bla: bool,
    /// Relative error tolerated per linearised step. 2^-24 visibly misplaces
    /// pixels near some minibrots; 2^-40 costs only a few percent more steps.
    pub bla_epsilon: f64,
    /// Track derivatives for distance estimation (slower).
    pub distance_estimate: bool,
    /// Stop iterating pixels that are attracted to a cycle (interior).
    pub interior_detection: bool,
    /// Keep using a previous reference orbit when it is still close enough.
    pub reuse_reference: bool,
    /// Choose the iteration limit automatically (see the module docs);
    /// `View::maxiter` is then only the starting hint.
    pub auto_iterations: bool,
    /// Upper bound for automatic limits.
    pub max_iterations: u64,
    /// Move the reference onto a minibrot nucleus near the view when there
    /// is one (see the module docs).
    pub periodic_reference: bool,
    /// Iterate four `f64` pixels at once with AVX2 where the CPU has it.
    /// Results are identical either way.
    pub simd: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1280,
            height: 720,
            bailout: 10000.0,
            bla: true,
            bla_epsilon: 1.0 / (1u64 << 40) as f64,
            distance_estimate: false,
            interior_detection: true,
            reuse_reference: true,
            auto_iterations: false,
            max_iterations: 1 << 28,
            periodic_reference: true,
            simd: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub reference: Duration,
    pub reference_reused: bool,
    pub reference_len: usize,
    pub reference_escaped: bool,
    /// The orbit stopped early (see `Reference::compute`).
    pub reference_truncated: bool,
    pub reference_limbs: usize,
    /// Lowest period of a minibrot found near the view, if any.
    pub nearby_period: Option<u64>,
    /// The reference sits on a minibrot nucleus rather than the view centre.
    pub reference_on_nucleus: bool,
    pub bla: Duration,
    pub bla_levels: usize,
    pub pixels: Duration,
    pub total: Duration,
    /// Sum of iteration counts over all pixels (and correction rounds).
    pub iterations: u64,
    /// Loop steps actually executed (a BLA skip counts as one).
    pub steps: u64,
    /// Of `steps`, those executed in extended range.
    pub deep_steps: u64,
    /// Pixels detected as interior.
    pub interior: u64,
    /// Pixels that reached the limit undecided.
    pub undecided: u64,
    pub min_escaped: u64,
    pub max_escaped: u64,
    /// Passes over the image: 1 plus the number of automatic corrections.
    pub rounds: u32,
    /// Iteration limit the next frame of a similar view should start from.
    pub suggested_maxiter: u64,
    /// "f64" or "floatexp+f64".
    pub mode: &'static str,
}

/// Raw per-pixel results; colouring happens separately.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// The iteration limit actually used.
    pub maxiter: u64,
    /// Integer part of the smooth iteration count, or [`INTERIOR`] / [`UNDECIDED`].
    pub n: Vec<u32>,
    /// Fractional part of the smooth iteration count, in `[0, 1)`.
    pub frac: Vec<f32>,
    /// Distance estimate in pixels (empty unless enabled).
    pub de: Vec<f32>,
    pub stats: Stats,
}

impl Frame {
    #[inline]
    pub fn escaped(&self, i: usize) -> bool {
        self.n[i] < UNDECIDED
    }

    /// Smooth iteration count of an escaped pixel.
    #[inline]
    pub fn mu(&self, i: usize) -> Option<f64> {
        self.escaped(i).then(|| self.n[i] as f64 + self.frac[i] as f64)
    }
}

/// BLA tables for the `f64` phase and, for deep zooms, the extended-range phase.
type Tables = (Arc<BlaTable<Complex<f64>>>, Option<Arc<BlaTable<CExp>>>);

struct BlaCache {
    key: (usize, usize, u64, u64),
    f64: Arc<BlaTable<Complex<f64>>>,
    deep: Option<Arc<BlaTable<CExp>>>,
}

/// Renders frames, caching the reference orbit and BLA tables between calls.
pub struct Renderer {
    reference: Option<Arc<Reference>>,
    bla: Option<BlaCache>,
    pool: Arc<rayon::ThreadPool>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Make subnormal floats flush to zero on this thread. Values that small are
/// always negligible here, and subnormal arithmetic can be very slow.
fn flush_subnormals() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: only sets the FTZ and DAZ bits of this thread's MXCSR.
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
        csr |= (1 << 15) | (1 << 6);
        std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, readonly));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: only sets the FZ bit of this thread's FPCR.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack));
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack));
    }
}

/// Everything the pixel loop needs that is fixed for one pass.
struct Pass<'a> {
    reference: &'a Reference,
    bla64: &'a BlaTable<Complex<f64>>,
    bla_deep: Option<&'a BlaTable<CExp>>,
    view: &'a View,
    width: u32,
    height: u32,
    offset: Complex<FloatExp>,
    spacing: FloatExp,
    deep: bool,
    bailout2: f64,
    log_bailout: f64,
    extras: Extras,
    limit: u64,
    simd: bool,
}

/// One pixel's stored result: `(n, frac, de)`.
type Out = (u32, f32, f32);

impl Pass<'_> {
    fn pixel(&self, x: u32, y: u32) -> PixelResult {
        let (px, py) = self.view.pixel_offset(x as f64 + 0.5, y as f64 + 0.5, self.width, self.height);
        if self.deep {
            let dc = self.offset + Complex::new(self.spacing * FloatExp::from(px), self.spacing * FloatExp::from(py));
            let deep_phase = Phase {
                bla: self.bla_deep.expect("deep BLA table"),
                dc: CExp::from_cfe(dc),
                spacing: self.spacing,
                bailout2: FloatExp::from(self.bailout2),
                switch2: perturb::switch_norm(),
            };
            let shallow_phase = Phase {
                bla: self.bla64,
                dc: Complex::new(perturb::flush_f64(dc.re), perturb::flush_f64(dc.im)),
                spacing: perturb::flush_f64(self.spacing),
                bailout2: self.bailout2,
                switch2: perturb::switch_norm(),
            };
            perturb::iterate_deep(self.reference, &deep_phase, &shallow_phase, self.limit, self.extras)
        } else {
            let phase = self.phase64(self.dc64(x, y));
            perturb::iterate_f64(self.reference, &phase, self.limit, self.extras)
        }
    }

    fn dc64(&self, x: u32, y: u32) -> Complex<f64> {
        let (px, py) = self.view.pixel_offset(x as f64 + 0.5, y as f64 + 0.5, self.width, self.height);
        let spacing = self.spacing.to_f64();
        self.offset.to_f64() + Complex::new(spacing * px, spacing * py)
    }

    fn phase64(&self, dc: Complex<f64>) -> Phase<'_, Complex<f64>> {
        Phase { bla: self.bla64, dc, spacing: self.spacing.to_f64(), bailout2: self.bailout2, switch2: 0.0 }
    }

    /// Iterate a list of pixels: batched (SIMD where possible) in `f64`
    /// mode, one by one for deep zooms.
    fn pixels(&self, coords: impl Iterator<Item = (u32, u32)>) -> Vec<PixelResult> {
        if self.deep {
            return coords.map(|(x, y)| self.pixel(x, y)).collect();
        }
        let dcs: Vec<Complex<f64>> = coords.map(|(x, y)| self.dc64(x, y)).collect();
        let mut out = vec![PixelResult::default(); dcs.len()];
        let phase = self.phase64(Complex::ZERO);
        perturb::iterate_f64_batch(self.reference, &phase, &dcs, self.limit, self.extras, self.simd, &mut out);
        out
    }

    fn store(&self, r: &PixelResult) -> Out {
        if r.escaped {
            let log_z = 0.5 * r.z.norm().ln();
            let mu = r.n as f64 + 1.0 - (log_z / self.log_bailout).log2();
            let fl = mu.floor();
            let de = if self.extras.distance_estimate { (r.z.abs() * log_z / r.der.abs()) as f32 } else { 0.0 };
            ((fl as u32).min(UNDECIDED - 1), (mu - fl) as f32, de)
        } else if r.n >= self.limit {
            (UNDECIDED, 0.0, 0.0)
        } else {
            (INTERIOR, 0.0, 0.0)
        }
    }
}

#[derive(Default)]
struct Counters {
    iterations: AtomicU64,
    steps: AtomicU64,
    deep_steps: AtomicU64,
}

impl Counters {
    fn add(&self, it: u64, st: u64, dst: u64) {
        self.iterations.fetch_add(it, Ordering::Relaxed);
        self.steps.fetch_add(st, Ordering::Relaxed);
        self.deep_steps.fetch_add(dst, Ordering::Relaxed);
    }
}

impl Renderer {
    pub fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("fracst-{i}"))
            .start_handler(|_| flush_subnormals())
            .build()
            .expect("render thread pool");
        Renderer { reference: None, bla: None, pool: Arc::new(pool) }
    }

    pub fn reference(&self) -> Option<&Arc<Reference>> {
        self.reference.as_ref()
    }

    pub fn clear_cache(&mut self) {
        self.reference = None;
        self.bla = None;
    }

    /// Render `view`; returns `None` if cancelled through `progress`.
    pub fn render(&mut self, view: &View, s: &Settings, progress: &Progress) -> Option<Frame> {
        let pool = self.pool.clone();
        pool.install(|| self.render_inner(view, s, progress))
    }

    fn render_inner(&mut self, view: &View, s: &Settings, progress: &Progress) -> Option<Frame> {
        let t_start = Instant::now();
        let (w, h) = (s.width.max(1), s.height.max(1));
        let pixels = w as usize * h as usize;
        let spacing = view.pixel_spacing(h);
        let half_diag = 0.5 * ((w as f64).powi(2) + (h as f64).powi(2)).sqrt() + 1.0;
        let auto = s.auto_iterations;
        let cap = s.max_iterations.clamp(1, UNDECIDED as u64 - 1);
        let floor = auto_floor(view.zoom).min(cap);
        let mut limit = if auto { view.maxiter.max(floor) } else { view.maxiter }.clamp(1, cap);
        let mut stats = Stats::default();

        // Reference orbit, reused when it is precise enough and not too far away.
        let t = Instant::now();
        progress.set_stage(Progress::REFERENCE, limit);
        let limbs = View::limbs_for(view.zoom, h);
        let offset_of = |r: &Reference| -> Complex<FloatExp> {
            let n = r.re.limbs().max(view.re.limbs());
            let dre = view.re.with_limbs(n).sub(&r.re.with_limbs(n));
            let dim = view.im.with_limbs(n).sub(&r.im.with_limbs(n));
            Complex::new(dre.to_floatexp(), dim.to_floatexp())
        };
        let stop_margin = spacing.ldexp(crate::reference::STOP_MARGIN_BITS);
        let reusable = self.reference.as_ref().is_some_and(|r| {
            s.reuse_reference
                && r.re.limbs() >= limbs
                && r.truncated.is_none_or(|t| t < stop_margin)
                && offset_of(r).abs() / spacing <= FloatExp::from(2.0 * w.max(h) as f64)
        });
        if reusable {
            stats.reference_reused = true;
        } else {
            self.reference = None;
            self.bla = None;
            // One spare limb lets the orbit be reused while zooming deeper.
            let n = limbs + 1;
            let (re, im) = (view.re.with_limbs(n), view.im.with_limbs(n));
            let probe = spacing * FloatExp::from(PROBE_RADII * half_diag);
            let mut r = None;
            if s.periodic_reference {
                // Within reuse distance of the view, so the orbit can be shared.
                let max_offset = spacing * FloatExp::from(2.0 * w.max(h) as f64);
                r = periodic_reference(&re, &im, limit, spacing, probe, max_offset, progress);
                if progress.cancelled() {
                    return None;
                }
            }
            let r = match r {
                Some(r) => r,
                None => Reference::compute(&re, &im, limit, spacing, probe, progress)?,
            };
            self.reference = Some(Arc::new(r));
        }
        stats.reference += t.elapsed();
        let (nearby_period, reference_escape) = {
            let r = self.reference.as_ref().unwrap();
            (r.nearby_period, r.escaped.then(|| r.last_index() as u64))
        };
        stats.nearby_period = nearby_period;
        if auto {
            // Leave room for the surroundings of a nearby minibrot, and for
            // pixels that escape a little later than the centre.
            let predicted = nearby_period.map_or(0, |p| p * PERIOD_FACTOR).max(reference_escape.map_or(0, |e| 2 * e));
            limit = limit.max(predicted).min(cap);
        }

        let offset = offset_of(self.reference.as_ref().unwrap());
        // |dc| bound over the image, for BLA validity radii.
        let cmax = offset.abs() + spacing * FloatExp::from(half_diag);
        let deep = spacing.exponent() < -900;
        stats.mode = if deep { "floatexp+f64" } else { "f64" };

        let mut frame = Frame {
            width: w,
            height: h,
            maxiter: limit,
            n: vec![UNDECIDED; pixels],
            frac: vec![0.0; pixels],
            de: if s.distance_estimate { vec![0.0; pixels] } else { Vec::new() },
            stats: Stats::default(),
        };
        let counters = Counters::default();
        let extras = Extras { distance_estimate: s.distance_estimate, interior_detection: s.interior_detection };
        // Pixels still to render: all of them at first, then the undecided ones.
        let mut todo: Option<Vec<u32>> = None;
        let mut rounds = 0;
        let mut previously_undecided = usize::MAX;
        loop {
            rounds += 1;
            let t = Instant::now();
            if !self.extend_reference(limit, progress)? {
                return None;
            }
            stats.reference += t.elapsed();
            let reference = self.reference.clone().unwrap();

            let t = Instant::now();
            progress.set_stage(Progress::BLA, 1);
            let (bla64, bla_deep) = self.bla_tables(&reference, cmax, deep, s);
            stats.bla += t.elapsed();
            stats.bla_levels = bla64.levels();
            if progress.cancelled() {
                return None;
            }

            let t = Instant::now();
            let pass = Pass {
                reference: &reference,
                bla64: &bla64,
                bla_deep: bla_deep.as_deref(),
                view,
                width: w,
                height: h,
                offset,
                spacing,
                deep,
                bailout2: s.bailout * s.bailout,
                log_bailout: s.bailout.ln(),
                extras,
                limit,
                simd: s.simd,
            };
            match &todo {
                None => render_all(&pass, &mut frame, &counters, progress),
                Some(list) => render_some(&pass, &mut frame, list, &counters, progress),
            }
            stats.pixels += t.elapsed();
            if progress.cancelled() {
                return None;
            }
            frame.maxiter = limit;
            if !auto || rounds > MAX_ROUNDS || limit >= cap {
                break;
            }
            // Raise the limit if pixels are still being cut off.
            let undecided: Vec<u32> = (0..pixels as u32).filter(|&i| frame.n[i as usize] == UNDECIDED).collect();
            if undecided.is_empty() {
                break;
            }
            let interior = frame.n.iter().filter(|&&n| n == INTERIOR).count();
            let tail = frame.n.iter().filter(|&&n| n < UNDECIDED && n as u64 > limit / 2).count();
            let threshold = (TAIL_FRACTION * pixels as f64).max(MIN_PIXELS);
            let cut_off =
                tail as f64 > threshold || (undecided.len() as f64 > threshold && undecided.len() > interior / 4);
            // Stop once a higher limit stops resolving pixels: what is left
            // converges too slowly to be worth chasing.
            let progressing = undecided.len() * 2 <= previously_undecided;
            if !cut_off || !progressing {
                break;
            }
            previously_undecided = undecided.len();
            limit = limit.saturating_mul(RAISE_FACTOR).min(cap);
            todo = Some(undecided);
        }

        // Statistics, and the limit to start the next frame from.
        let mut escapes: Vec<u32> = frame.n.iter().copied().filter(|&n| n < UNDECIDED).collect();
        stats.interior = frame.n.iter().filter(|&&n| n == INTERIOR).count() as u64;
        stats.undecided = frame.n.iter().filter(|&&n| n == UNDECIDED).count() as u64;
        stats.min_escaped = escapes.iter().copied().min().map_or(0, u64::from);
        stats.max_escaped = escapes.iter().copied().max().map_or(0, u64::from);
        stats.suggested_maxiter = if auto {
            let robust_max = if escapes.is_empty() {
                0
            } else {
                let k = ((escapes.len() as f64 * (1.0 - TAIL_FRACTION)) as usize).min(escapes.len() - 1);
                *escapes.select_nth_unstable(k).1 as u64
            };
            let threshold = (TAIL_FRACTION * pixels as f64).max(MIN_PIXELS);
            let still_cut_off = stats.undecided as f64 > threshold && stats.undecided > stats.interior / 4;
            let mut next = floor.max(2 * robust_max).max(nearby_period.map_or(0, |p| p * PERIOD_FACTOR));
            if still_cut_off {
                next = next.max(limit);
            }
            next.min(cap)
        } else {
            view.maxiter
        };
        let reference = self.reference.as_ref().unwrap();
        stats.reference_len = reference.last_index();
        stats.reference_escaped = reference.escaped;
        stats.reference_truncated = reference.truncated.is_some();
        stats.reference_limbs = reference.re.limbs();
        stats.reference_on_nucleus = reference.re != view.re.with_limbs(reference.re.limbs())
            || reference.im != view.im.with_limbs(reference.im.limbs());
        stats.rounds = rounds;
        stats.iterations = counters.iterations.into_inner();
        stats.steps = counters.steps.into_inner();
        stats.deep_steps = counters.deep_steps.into_inner();
        stats.total = t_start.elapsed();
        frame.stats = stats;
        progress.set_stage(Progress::IDLE, 0);
        Some(frame)
    }

    /// Make the cached reference cover `limit` iterations. `None` if cancelled.
    fn extend_reference(&mut self, limit: u64, progress: &Progress) -> Option<bool> {
        let arc = self.reference.as_mut().expect("reference");
        if arc.covers(limit) {
            return Some(true);
        }
        progress.set_stage(Progress::REFERENCE, limit);
        let r = Arc::get_mut(arc).expect("the reference orbit is only shared during a render");
        Some(r.extend(limit, progress))
    }

    fn bla_tables(&mut self, reference: &Arc<Reference>, cmax: FloatExp, deep: bool, s: &Settings) -> Tables {
        if !s.bla {
            return (Arc::new(BlaTable::empty()), deep.then(|| Arc::new(BlaTable::empty())));
        }
        let len = reference.last_index();
        let key =
            (Arc::as_ptr(reference) as usize, len, f64::to_bits(cmax.log2()), s.bla_epsilon.to_bits() ^ deep as u64);
        if let Some(c) = self.bla.as_ref().filter(|c| c.key == key) {
            return (c.f64.clone(), c.deep.clone());
        }
        let (t64, tdeep) = if deep {
            let fe = BlaTable::<Complex<FloatExp>>::build(len, |m| reference.z_fe(m), cmax, s.bla_epsilon);
            (Arc::new(fe.cast()), Some(Arc::new(fe.cast())))
        } else {
            let t = BlaTable::<Complex<f64>>::build(len, |m| reference.z(m), cmax.to_f64(), s.bla_epsilon);
            (Arc::new(t), None)
        };
        self.bla = Some(BlaCache { key, f64: t64.clone(), deep: tdeep.clone() });
        (t64, tdeep)
    }
}

/// A reference on the nucleus of a minibrot near `(re, im)`: the lowest-period
/// one within `probe`, if Newton's method finds it within `max_offset` and its
/// orbit stops at the period for this pixel spacing.
fn periodic_reference(
    re: &Fixed,
    im: &Fixed,
    limit: u64,
    spacing: FloatExp,
    probe: FloatExp,
    max_offset: FloatExp,
    progress: &Progress,
) -> Option<Reference> {
    let period = newton::find_period(re, im, probe, limit, progress)?;
    // Newton steps gain roughly 15 digits each with an f64-precision derivative.
    let steps = 16 + re.precision_bits() as usize / 40;
    let (nr, ni) = newton::find_nucleus(re, im, period, steps, progress)?;
    let offset = Complex::new(nr.sub(re).to_floatexp(), ni.sub(im).to_floatexp()).abs();
    if offset > max_offset {
        return None;
    }
    let r = Reference::compute(&nr, &ni, limit, spacing, probe, progress)?;
    r.truncated.is_some().then_some(r)
}

/// Render every pixel, a row at a time.
fn render_all(pass: &Pass, frame: &mut Frame, counters: &Counters, progress: &Progress) {
    let w = frame.width as usize;
    progress.set_stage(Progress::PIXELS, frame.height as u64);
    let mut de_rows: Vec<&mut [f32]> = if frame.de.is_empty() {
        (0..frame.height).map(|_| &mut [][..]).collect()
    } else {
        frame.de.chunks_mut(w).collect()
    };
    frame.n.par_chunks_mut(w).zip(frame.frac.par_chunks_mut(w)).zip(de_rows.par_iter_mut()).enumerate().for_each(
        |(y, ((row_n, row_frac), row_de))| {
            if progress.cancelled() {
                return;
            }
            let (mut it, mut st, mut dst) = (0, 0, 0);
            let results = pass.pixels((0..w as u32).map(|x| (x, y as u32)));
            for (x, r) in results.iter().enumerate() {
                (it, st, dst) = (it + r.n, st + r.steps, dst + r.deep_steps);
                let (n, frac, de) = pass.store(r);
                row_n[x] = n;
                row_frac[x] = frac;
                if let Some(d) = row_de.get_mut(x) {
                    *d = de;
                }
            }
            counters.add(it, st, dst);
            progress.done.fetch_add(1, Ordering::Relaxed);
        },
    );
}

/// Render only the listed pixels (indices into the frame).
fn render_some(pass: &Pass, frame: &mut Frame, list: &[u32], counters: &Counters, progress: &Progress) {
    const CHUNK: usize = 64;
    let w = frame.width;
    progress.set_stage(Progress::PIXELS, list.len().div_ceil(CHUNK) as u64);
    let results: Vec<(u32, Out)> = list
        .par_chunks(CHUNK)
        .flat_map_iter(|chunk| {
            let (mut it, mut st, mut dst) = (0, 0, 0);
            let out: Vec<(u32, Out)> = if progress.cancelled() {
                Vec::new()
            } else {
                let results = pass.pixels(chunk.iter().map(|&i| (i % w, i / w)));
                chunk
                    .iter()
                    .zip(&results)
                    .map(|(&i, r)| {
                        (it, st, dst) = (it + r.n, st + r.steps, dst + r.deep_steps);
                        (i, pass.store(r))
                    })
                    .collect()
            };
            counters.add(it, st, dst);
            progress.done.fetch_add(1, Ordering::Relaxed);
            out
        })
        .collect();
    for (i, (n, frac, de)) in results {
        let i = i as usize;
        frame.n[i] = n;
        frame.frac[i] = frac;
        if let Some(d) = frame.de.get_mut(i) {
            *d = de;
        }
    }
}
