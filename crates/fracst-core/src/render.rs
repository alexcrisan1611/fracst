//! Frame rendering: reference orbit, BLA table and the parallel pixel loop.

use crate::Progress;
use crate::bla::BlaTable;
use crate::floatexp::FloatExp;
use crate::num::{CExp, Complex, Cplx};
use crate::perturb::{self, Extras, Phase, PixelResult};
use crate::reference::Reference;
use crate::view::View;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Marks pixels that did not escape within the iteration limit.
pub const INTERIOR: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    /// Escape radius. Large values give smoother colouring.
    pub bailout: f64,
    pub bla: bool,
    /// Relative error tolerated per linearised step. Fraktaler 3 uses 2^-24,
    /// which visibly misplaces pixels near some minibrots; 2^-40 costs a few
    /// percent more steps.
    pub bla_epsilon: f64,
    /// Track derivatives for distance estimation (slower).
    pub distance_estimate: bool,
    /// Stop iterating pixels that are attracted to a cycle (interior).
    pub interior_detection: bool,
    /// Keep using a previous reference orbit when it is still close enough.
    pub reuse_reference: bool,
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
    pub bla: Duration,
    pub bla_levels: usize,
    pub pixels: Duration,
    pub total: Duration,
    /// Sum of iteration counts over all pixels.
    pub iterations: u64,
    /// Loop steps actually executed (a BLA skip counts as one).
    pub steps: u64,
    /// Of `steps`, those executed in extended range.
    pub deep_steps: u64,
    pub interior: u64,
    pub min_escaped: u64,
    pub max_escaped: u64,
    /// "f64" or "floatexp+f64".
    pub mode: &'static str,
}

/// Raw per-pixel results; colouring happens separately.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub maxiter: u64,
    /// Integer part of the smooth iteration count, or [`INTERIOR`].
    pub n: Vec<u32>,
    /// Fractional part of the smooth iteration count, in `[0, 1)`.
    pub frac: Vec<f32>,
    /// Distance estimate in pixels (empty unless enabled).
    pub de: Vec<f32>,
    pub stats: Stats,
}

impl Frame {
    #[inline]
    pub fn mu(&self, i: usize) -> Option<f64> {
        let n = self.n[i];
        (n != INTERIOR).then(|| n as f64 + self.frac[i] as f64)
    }
}

struct BlaCache {
    key: (usize, u64, u64),
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
        let maxiter = view.maxiter.clamp(1, INTERIOR as u64 - 1);
        let spacing = view.pixel_spacing(h);
        let half_diag = 0.5 * ((w as f64).powi(2) + (h as f64).powi(2)).sqrt() + 1.0;
        let mut stats = Stats::default();

        // Reference orbit, reused when it is precise and long enough and not too far away.
        progress.set_stage(Progress::REFERENCE, maxiter);
        let limbs = View::limbs_for(view.zoom, h);
        let offset_of = |r: &Reference| -> Complex<FloatExp> {
            let n = r.re.limbs().max(view.re.limbs());
            let dre = view.re.with_limbs(n).sub(&r.re.with_limbs(n));
            let dim = view.im.with_limbs(n).sub(&r.im.with_limbs(n));
            Complex::new(dre.to_floatexp(), dim.to_floatexp())
        };
        let stop_margin = spacing.ldexp(crate::reference::STOP_MARGIN_BITS);
        let reusable = self.reference.as_ref().filter(|r| {
            s.reuse_reference
                && r.re.limbs() >= limbs
                && (r.escaped || r.truncated.map_or(r.maxiter >= maxiter, |t| t < stop_margin))
                && offset_of(r).abs() / spacing <= FloatExp::from(2.0 * w.max(h) as f64)
        });
        let reference = match reusable {
            Some(r) => {
                stats.reference_reused = true;
                r.clone()
            }
            None => {
                self.reference = None;
                self.bla = None;
                let t = Instant::now();
                // One spare limb lets the orbit be reused while zooming deeper.
                let n = limbs + 1;
                let r = Arc::new(Reference::compute(
                    &view.re.with_limbs(n),
                    &view.im.with_limbs(n),
                    maxiter,
                    spacing,
                    progress,
                )?);
                stats.reference = t.elapsed();
                self.reference = Some(r.clone());
                r
            }
        };
        stats.reference_len = reference.last_index();
        stats.reference_escaped = reference.escaped;
        stats.reference_truncated = reference.truncated.is_some();
        stats.reference_limbs = reference.re.limbs();
        let offset = offset_of(&reference);

        // |dc| bound over the image, for BLA validity radii.
        let cmax = offset.abs() + spacing * FloatExp::from(half_diag);
        let deep = spacing.exponent() < -900;
        stats.mode = if deep { "floatexp+f64" } else { "f64" };

        progress.set_stage(Progress::BLA, 1);
        let t = Instant::now();
        let (bla64, bla_deep) = if s.bla {
            let key =
                (Arc::as_ptr(&reference) as usize, f64::to_bits(cmax.log2()), s.bla_epsilon.to_bits() ^ deep as u64);
            match &self.bla {
                Some(c) if c.key == key => (c.f64.clone(), c.deep.clone()),
                _ => {
                    let len = reference.last_index();
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
        } else {
            (Arc::new(BlaTable::empty()), deep.then(|| Arc::new(BlaTable::empty())))
        };
        stats.bla = t.elapsed();
        stats.bla_levels = bla64.levels();
        if progress.cancelled() {
            return None;
        }

        // Pixels.
        progress.set_stage(Progress::PIXELS, h as u64);
        let t = Instant::now();
        let bailout2 = s.bailout * s.bailout;
        let log_bailout = s.bailout.ln();
        let mut frame = Frame {
            width: w,
            height: h,
            maxiter,
            n: vec![0; (w * h) as usize],
            frac: vec![0.0; (w * h) as usize],
            de: if s.distance_estimate { vec![0.0; (w * h) as usize] } else { Vec::new() },
            stats: Stats::default(),
        };
        let iterations = AtomicU64::new(0);
        let steps = AtomicU64::new(0);
        let deep_steps = AtomicU64::new(0);
        let interior = AtomicU64::new(0);
        let min_escaped = AtomicU64::new(u64::MAX);
        let max_escaped = AtomicU64::new(0);
        let spacing64 = spacing.to_f64();
        let offset64 = offset.to_f64();
        let de_row_len = if s.distance_estimate { w as usize } else { 0 };
        let extras = Extras { distance_estimate: s.distance_estimate, interior_detection: s.interior_detection };
        let mut de_rows: Vec<&mut [f32]> = if s.distance_estimate {
            frame.de.chunks_mut(w as usize).collect()
        } else {
            (0..h).map(|_| &mut [][..]).collect()
        };

        frame
            .n
            .par_chunks_mut(w as usize)
            .zip(frame.frac.par_chunks_mut(w as usize))
            .zip(de_rows.par_iter_mut())
            .enumerate()
            .for_each(|(y, ((row_n, row_frac), row_de))| {
                if progress.cancelled() {
                    return;
                }
                let (mut it, mut st, mut dst, mut inside, mut lo, mut hi) = (0u64, 0u64, 0u64, 0u64, u64::MAX, 0u64);
                for x in 0..w as usize {
                    let (px, py) = view.pixel_offset(x as f64 + 0.5, y as f64 + 0.5, w, h);
                    let r: PixelResult = if deep {
                        let dc = offset + Complex::new(spacing * FloatExp::from(px), spacing * FloatExp::from(py));
                        let deep_phase = Phase {
                            bla: bla_deep.as_deref().unwrap(),
                            dc: CExp::from_cfe(dc),
                            spacing,
                            bailout2: FloatExp::from(bailout2),
                            switch2: perturb::switch_norm(),
                        };
                        let shallow_phase = Phase {
                            bla: &*bla64,
                            dc: Complex::new(perturb::flush_f64(dc.re), perturb::flush_f64(dc.im)),
                            spacing: perturb::flush_f64(spacing),
                            bailout2,
                            switch2: perturb::switch_norm(),
                        };
                        perturb::iterate_deep(&reference, &deep_phase, &shallow_phase, maxiter, extras)
                    } else {
                        let dc = offset64 + Complex::new(spacing64 * px, spacing64 * py);
                        let phase = Phase { bla: &*bla64, dc, spacing: spacing64, bailout2, switch2: 0.0 };
                        perturb::iterate_f64(&reference, &phase, maxiter, extras)
                    };
                    it += r.n;
                    st += r.steps;
                    dst += r.deep_steps;
                    if r.escaped {
                        let log_z = 0.5 * r.z.norm().ln();
                        let mu = r.n as f64 + 1.0 - (log_z / log_bailout).log2();
                        let fl = mu.floor();
                        row_n[x] = fl as u32;
                        row_frac[x] = (mu - fl) as f32;
                        lo = lo.min(r.n);
                        hi = hi.max(r.n);
                        if de_row_len > 0 {
                            row_de[x] = (r.z.abs() * log_z / r.der.abs()) as f32;
                        }
                    } else {
                        row_n[x] = INTERIOR;
                        row_frac[x] = 0.0;
                        inside += 1;
                    }
                }
                iterations.fetch_add(it, Ordering::Relaxed);
                steps.fetch_add(st, Ordering::Relaxed);
                deep_steps.fetch_add(dst, Ordering::Relaxed);
                interior.fetch_add(inside, Ordering::Relaxed);
                min_escaped.fetch_min(lo, Ordering::Relaxed);
                max_escaped.fetch_max(hi, Ordering::Relaxed);
                progress.done.fetch_add(1, Ordering::Relaxed);
            });
        drop(de_rows);
        if progress.cancelled() {
            return None;
        }
        stats.pixels = t.elapsed();
        stats.iterations = iterations.into_inner();
        stats.steps = steps.into_inner();
        stats.deep_steps = deep_steps.into_inner();
        stats.interior = interior.into_inner();
        stats.min_escaped = min_escaped.into_inner();
        stats.max_escaped = max_escaped.into_inner();
        stats.total = t_start.elapsed();
        frame.stats = stats;
        progress.set_stage(Progress::IDLE, 0);
        Some(frame)
    }
}
