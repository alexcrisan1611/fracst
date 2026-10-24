//! Per-pixel perturbation iteration with BLA skipping and rebasing.
//!
//! A pixel at `C + dc` iterates the offset `dz` from the reference orbit `Z`:
//! `dz' = (2 Z_m + dz) dz + dc`. When the pixel orbit gets closer to zero than
//! to the reference (`|Z_m + dz| < |dz|`), or the reference runs out, the pixel
//! is *rebased* onto the start of the reference (`dz = Z_m + dz`, `m = 0`),
//! which removes the need for glitch detection and secondary references.
//!
//! Deep zooms start in extended range ([`CExp`]) while `|dz|` is tiny, then
//! drop to plain `f64` once `|dz|` is large enough, coming back only if it
//! shrinks again.
//!
//! Optional extras: the derivative `dz/dc` (scaled by the pixel spacing) for
//! distance estimation, and interior detection, which tracks `dz_n/dz_1` and
//! stops once it falls below [`INTERIOR_RADIUS2`] (the pixel is attracted to
//! a cycle).

use crate::bla::BlaTable;
use crate::floatexp::FloatExp;
use crate::num::{CExp, Complex, Cplx, Real};
use crate::reference::Reference;

/// `|dz|^2` above which a deep pixel switches to `f64` (`|dz| > 2^-480`). At
/// that size `dz^2` and `2 Z dz` stay normal doubles, and a dropped `dc` below
/// 2^-1000 is negligible next to them.
const SWITCH_NORM: f64 = f64::from_bits((1023 - 960) << 52);

/// Interior detection threshold on the squared derivative of the orbit with
/// respect to an earlier iterate (an inscape radius of 1/1024).
pub const INTERIOR_RADIUS2: f64 = 1.0 / (1024.0 * 1024.0);

/// Interior detection comes in two flavours:
///
/// * With a periodic reference (centred on a minibrot nucleus), every pixel
///   rebases at the end of each period. From the first period end after
///   `INTERIOR_START` the derivative is accumulated and checked only at
///   period ends, where it is the cycle multiplier `lambda^k`; two
///   consecutive checks below the threshold mean the pixel is attracted to
///   the cycle. Checking only whole periods matters: mid-period, exterior
///   orbits dip far below the threshold as they pass near zero, and deep
///   minibrots grow the derivative by huge factors before it settles.
/// * Otherwise `|dz_n/dz_1|` is checked every iteration.
///   With high limits pixels first run to `INTERIOR_START` without it; only
///   those still going restart from `z_1` with it (cheap with BLA), so most
///   exterior pixels never pay for the extra derivative.
const INTERIOR_START: u64 = 4096;

/// Rescaling of `dz_n/dz_1` in `f64`, which can exceed the range of doubles.
const DW_BIG: f64 = f64::from_bits((1023 + 600) << 52);
const DW_SMALL: f64 = f64::from_bits((1023 - 600) << 52);
const DW_STEP: i32 = 300;

/// `|Z_m|^2` above which an orbit value dwarfs any extended-range `dz`
/// (`|dz| <= 2^-480`), so `Z_m + dz` cannot trigger a rebase.
const ORDINARY_NORM: f64 = f64::from_bits((1023 - 900) << 52);

/// Per-type constants of one iteration phase.
pub struct Phase<'a, C: Cplx> {
    pub bla: &'a BlaTable<C>,
    pub dc: C,
    /// Pixel spacing, for the scaled derivative `dz/dc * spacing`.
    pub spacing: C::R,
    pub bailout2: C::R,
    pub switch2: C::R,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PixelResult {
    /// Escape iteration (or where iteration stopped for points that did not escape).
    pub n: u64,
    pub escaped: bool,
    /// `z` at escape.
    pub z: Complex<f64>,
    /// `dz/dc` scaled by the pixel spacing (only with distance estimation).
    pub der: Complex<f64>,
    /// Loop iterations actually executed (BLA steps count once).
    pub steps: u64,
    /// Of `steps`, those executed in extended range.
    pub deep_steps: u64,
}

/// What to compute besides the escape time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub distance_estimate: bool,
    pub interior_detection: bool,
}

#[derive(Clone, Copy)]
struct State<C: Cplx> {
    dz: C,
    der: C,
    /// Derivative for interior detection, times `2^dw_exp`, once `dw_active`.
    dw: C,
    dw_exp: i32,
    dw_active: bool,
    /// Consecutive period ends with `dw` below the threshold.
    dw_below: u8,
    /// Period-end detection activates at the first period end from here.
    dw_start: u64,
    m: usize,
    n: u64,
    steps: u64,
}

impl<C: Cplx> State<C> {
    /// The state after the first iteration: `z_1 = c`, so `dz_1 = dc`.
    /// `dw_active` starts on for per-iteration (`z_1`-based) detection and
    /// off for the period-end flavour, which activates later.
    fn start(dc: C, spacing: C::R, dw_active: bool, maxiter: u64) -> Self {
        State {
            dz: dc,
            der: C::from_real(spacing),
            dw: C::ONE,
            dw_exp: 0,
            dw_active,
            dw_below: 0,
            dw_start: if maxiter > 2 * INTERIOR_START { INTERIOR_START } else { 0 },
            m: 1,
            n: 1,
            steps: 0,
        }
    }

    fn cast<D: Cplx>(&self) -> State<D> {
        // Bring dw into range of the target type by moving powers of two into dw_exp.
        let dw = self.dw.to_cfe();
        let mag = dw.re.abs().max(dw.im.abs());
        let shift = if mag.is_zero() { 0 } else { ((mag.exponent() - 500).max(0) + DW_STEP - 1) / DW_STEP * DW_STEP };
        let dw = Complex::new(dw.re.ldexp(-shift), dw.im.ldexp(-shift));
        State {
            dz: D::from_cfe(self.dz.to_cfe()),
            der: D::from_cfe(self.der.to_cfe()),
            dw: D::from_cfe(dw),
            dw_exp: self.dw_exp + shift,
            dw_active: self.dw_active,
            dw_below: self.dw_below,
            dw_start: self.dw_start,
            m: self.m,
            n: self.n,
            steps: self.steps,
        }
    }
}

enum Exit {
    Escaped(Complex<f64>),
    Stopped,
    Switch,
    /// One iteration done (only with `ONE`).
    Yield,
}

/// Iterate until escape, `maxiter`, interior, or (if `UP`/`DOWN`) `|dz|^2`
/// crossing `switch2`.
///
/// `DEEP` enables the extended-range optimisations, with `zn_at(m)` giving
/// `|Z_m|^2` in `f64`. While `|dz|^2 <= switch2` and `Z_m` is neither tiny
/// nor near the bailout, `Z_m + dz` can neither escape nor rebase, so those
/// tests (and the extended-range sum and norm behind them) are skipped; and
/// BLA lookups search from the longest step down, as long skips are the
/// norm while `dz` is tiny. The results are the same either way.
#[inline(always)]
fn run<
    C: Cplx,
    const DE: bool,
    const INSIDE: bool,
    const UP: bool,
    const DOWN: bool,
    const DEEP: bool,
    const FMA: bool,
    const ONE: bool,
>(
    reference: &Reference,
    z_at: impl Fn(usize) -> C,
    zn_at: impl Fn(usize) -> f64,
    p: &Phase<C>,
    st: &mut State<C>,
    maxiter: u64,
) -> Exit {
    let mref = reference.last_index();
    let periodic = reference.truncated.is_some();
    let fast_path = DEEP;
    let State { mut dz, mut der, mut dw, mut dw_exp, mut dw_active, mut dw_below, dw_start, mut m, mut n, mut steps } =
        *st;
    let ir2 = C::R::from_f64(INTERIOR_RADIUS2);
    let (dw_big, dw_small) = (C::R::from_f64(DW_BIG), C::R::from_f64(DW_SMALL));
    let mut zm = z_at(m);
    // Below a quarter of the bailout, |Z_m + dz| stays inside the escape radius.
    let zn_escape = if fast_path { p.bailout2.to_fe().to_f64() * 0.25 } else { 0.0 };
    // With `FMA`, multiply-adds are fused: shorter dependency chains per step.
    let mul_add = |a: C, b: C, c: C| if FMA { a.fused_mul_add(b, c) } else { a * b + c };
    let norm = |a: C| if FMA { a.fused_norm() } else { a.norm() };
    let exit = loop {
        let mut dzn = norm(dz);
        let fast = fast_path && m != mref && !(dzn > p.switch2) && {
            let zmn = zn_at(m);
            zmn >= ORDINARY_NORM && zmn <= zn_escape
        };
        let mut z = zm;
        if !fast {
            z = zm + dz;
            let zn = norm(z);
            if zn > p.bailout2 {
                break Exit::Escaped(z.to_c64());
            }
            if zn < dzn || m == mref {
                if INSIDE && periodic && m == mref {
                    // End of a period of the reference cycle.
                    if dw_active {
                        dw_below = if dw_exp <= 0 && dw.norm() < ir2 { dw_below + 1 } else { 0 };
                        if dw_below >= 2 {
                            break Exit::Stopped;
                        }
                    } else if n >= dw_start {
                        dw_active = true;
                        dw = C::ONE;
                        dw_exp = 0;
                    }
                }
                dz = z;
                dzn = zn;
                m = 0;
                zm = C::ZERO;
                z = dz;
            }
        }
        if n >= maxiter {
            break Exit::Stopped;
        }
        if (UP && dzn > p.switch2) || (DOWN && dzn < p.switch2) {
            break Exit::Switch;
        }
        if fast_path && INSIDE && dw_active {
            // Extended range cannot overflow (`dw_exp` is folded in on entry),
            // so only the per-iteration test needs the norm; periodic
            // references check at period ends instead.
            if !periodic && dw.norm() < ir2 {
                break Exit::Stopped;
            }
        } else if INSIDE && dw_active {
            let dwn = dw.norm();
            if !periodic && dw_exp == 0 && dwn < ir2 {
                break Exit::Stopped;
            }
            if dwn > dw_big {
                dw = dw.scale(C::R::from_f64(1.0 / DW_BIG.sqrt()));
                dw_exp += DW_STEP;
            } else if dw_exp > 0 && dwn < dw_small {
                dw = dw.scale(C::R::from_f64(DW_BIG.sqrt()));
                dw_exp -= DW_STEP;
            }
        }
        steps += 1;
        let bla = if DEEP { p.bla.lookup_longest(m, dzn, maxiter - n) } else { p.bla.lookup(m, dzn, maxiter - n) };
        if let Some(b) = bla {
            dz = mul_add(b.a, dz, b.b * p.dc);
            if DE {
                der = mul_add(b.a, der, b.b.scale(p.spacing));
            }
            if INSIDE && dw_active {
                dw = b.a * dw;
            }
            m += b.l as usize;
            n += b.l as u64;
        } else {
            if DE || (INSIDE && dw_active) {
                let z2 = if fast { zm + dz } else { z }.mul2();
                if DE {
                    der = mul_add(z2, der, C::from_real(p.spacing));
                }
                if INSIDE && dw_active {
                    dw = z2 * dw;
                }
            }
            dz = mul_add(zm.mul2() + dz, dz, p.dc);
            m += 1;
            n += 1;
        }
        zm = z_at(m);
        if ONE {
            break Exit::Yield;
        }
    };
    *st = State { dz, der, dw, dw_exp, dw_active, dw_below, dw_start, m, n, steps };
    exit
}

fn finish<C: Cplx>(st: &State<C>, exit: Exit, deep_steps: u64) -> PixelResult {
    let (escaped, z) = match exit {
        Exit::Escaped(z) => (true, z),
        _ => (false, Complex::ZERO),
    };
    PixelResult { n: st.n, escaped, z, der: st.der.to_c64(), steps: st.steps, deep_steps }
}

macro_rules! dispatch {
    ($extras:expr, $f:ident :: <_, _ $(, $rest:tt)*> ($($arg:expr),*)) => {
        match ($extras.distance_estimate, $extras.interior_detection) {
            (false, false) => $f::<false, false $(, $rest)*>($($arg),*),
            (false, true) => $f::<false, true $(, $rest)*>($($arg),*),
            (true, false) => $f::<true, false $(, $rest)*>($($arg),*),
            (true, true) => $f::<true, true $(, $rest)*>($($arg),*),
        }
    };
}

/// Iterate one pixel entirely in `f64` (zooms shallower than ~1e-270).
///
/// On CPUs with fused multiply-add this runs a copy of the loop compiled
/// with FMA enabled, which shortens each step's dependency chain.
pub fn iterate_f64(reference: &Reference, p: &Phase<Complex<f64>>, maxiter: u64, extras: Extras) -> PixelResult {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("fma") {
            // SAFETY: the CPU supports FMA, checked just above.
            return unsafe { iterate_f64_fma(reference, p, maxiter, extras) };
        }
        dispatch!(extras, iterate_f64_impl::<_, _, false>(reference, p, maxiter))
    }
    // FMA is part of the base AArch64 instruction set.
    #[cfg(target_arch = "aarch64")]
    {
        dispatch!(extras, iterate_f64_impl::<_, _, true>(reference, p, maxiter))
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        dispatch!(extras, iterate_f64_impl::<_, _, false>(reference, p, maxiter))
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
fn iterate_f64_fma(reference: &Reference, p: &Phase<Complex<f64>>, maxiter: u64, extras: Extras) -> PixelResult {
    dispatch!(extras, iterate_f64_impl::<_, _, true>(reference, p, maxiter))
}

#[inline(always)]
fn iterate_f64_impl<const DE: bool, const INSIDE: bool, const FMA: bool>(
    reference: &Reference,
    p: &Phase<Complex<f64>>,
    maxiter: u64,
) -> PixelResult {
    let z_at = |m| reference.z(m);
    let periodic = reference.truncated.is_some();
    let mut steps = 0;
    if INSIDE && !periodic && maxiter > 2 * INTERIOR_START {
        let mut st = State::start(p.dc, p.spacing, false, maxiter);
        let exit =
            run::<_, DE, false, false, false, false, FMA, false>(reference, z_at, |_| 0.0, p, &mut st, INTERIOR_START);
        if !matches!(exit, Exit::Stopped) {
            return finish(&st, exit, 0);
        }
        steps = st.steps;
    }
    let mut st = State::start(p.dc, p.spacing, !periodic, maxiter);
    st.steps = steps;
    let exit = run::<_, DE, INSIDE, false, false, false, FMA, false>(reference, z_at, |_| 0.0, p, &mut st, maxiter);
    finish(&st, exit, 0)
}

/// Iterate many pixels entirely in `f64`, one per entry of `dcs`, into `out`.
/// `p.dc` is ignored. With `simd` on a CPU with AVX2 and FMA (and without
/// distance estimation) four pixels advance together; the results are the
/// same as [`iterate_f64`] gives for each pixel.
pub fn iterate_f64_batch(
    reference: &Reference,
    p: &Phase<Complex<f64>>,
    dcs: &[Complex<f64>],
    maxiter: u64,
    extras: Extras,
    simd: bool,
    out: &mut [PixelResult],
) {
    assert_eq!(dcs.len(), out.len());
    #[cfg(target_arch = "x86_64")]
    if simd
        && !extras.distance_estimate
        && std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("fma")
    {
        // SAFETY: AVX2 and FMA are available, checked just above.
        unsafe {
            if extras.interior_detection {
                avx2::batch::<true>(reference, p, dcs, maxiter, out);
            } else {
                avx2::batch::<false>(reference, p, dcs, maxiter, out);
            }
        }
        return;
    }
    let _ = simd;
    for (dc, r) in dcs.iter().zip(out.iter_mut()) {
        let phase = Phase { bla: p.bla, dc: *dc, spacing: p.spacing, bailout2: p.bailout2, switch2: p.switch2 };
        *r = iterate_f64(reference, &phase, maxiter, extras);
    }
}

/// Iterate one pixel of a deep zoom: [`CExp`] while `|dz|` is tiny, `f64` otherwise.
pub fn iterate_deep(
    reference: &Reference,
    deep: &Phase<CExp>,
    shallow: &Phase<Complex<f64>>,
    maxiter: u64,
    extras: Extras,
) -> PixelResult {
    dispatch!(extras, iterate_deep_impl::<_, _>(reference, deep, shallow, maxiter))
}

#[inline(always)]
fn iterate_deep_impl<const DE: bool, const INSIDE: bool>(
    reference: &Reference,
    deep: &Phase<CExp>,
    shallow: &Phase<Complex<f64>>,
    maxiter: u64,
) -> PixelResult {
    let periodic = reference.truncated.is_some();
    let mut deep_steps = 0;
    let mut steps = 0;
    if INSIDE && !periodic && maxiter > 2 * INTERIOR_START {
        let mut st = Hybrid::Deep(State::start(deep.dc, deep.spacing, false, maxiter));
        let exit = run_hybrid::<DE, false>(reference, deep, shallow, &mut st, INTERIOR_START, &mut deep_steps);
        if !matches!(exit, Exit::Stopped) {
            return st.finish(exit, deep_steps);
        }
        steps = st.steps();
    }
    let mut st = Hybrid::Deep(State { steps, ..State::start(deep.dc, deep.spacing, !periodic, maxiter) });
    let exit = run_hybrid::<DE, INSIDE>(reference, deep, shallow, &mut st, maxiter, &mut deep_steps);
    st.finish(exit, deep_steps)
}

/// A deep pixel's state in whichever representation it currently uses.
enum Hybrid {
    Deep(State<CExp>),
    Shallow(State<Complex<f64>>),
}

impl Hybrid {
    fn steps(&self) -> u64 {
        match self {
            Hybrid::Deep(s) => s.steps,
            Hybrid::Shallow(s) => s.steps,
        }
    }

    fn finish(&self, exit: Exit, deep_steps: u64) -> PixelResult {
        match self {
            Hybrid::Deep(s) => finish(s, exit, deep_steps),
            Hybrid::Shallow(s) => finish(s, exit, deep_steps),
        }
    }
}

/// Alternate between extended range and `f64` until escape or `maxiter`.
#[inline(always)]
fn run_hybrid<const DE: bool, const INSIDE: bool>(
    reference: &Reference,
    deep: &Phase<CExp>,
    shallow: &Phase<Complex<f64>>,
    st: &mut Hybrid,
    maxiter: u64,
    deep_steps: &mut u64,
) -> Exit {
    loop {
        match st {
            Hybrid::Deep(s) => {
                let before = s.steps;
                let orbit = reference.deep_orbit();
                let exit = run::<_, DE, INSIDE, true, false, true, false, false>(
                    reference,
                    |m| orbit[m].z,
                    |m| orbit[m].norm,
                    deep,
                    s,
                    maxiter,
                );
                *deep_steps += s.steps - before;
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                *st = Hybrid::Shallow(s.cast());
            }
            Hybrid::Shallow(s) => {
                let exit = run::<_, DE, INSIDE, false, true, false, false, false>(
                    reference,
                    |m| reference.z(m),
                    |_| 0.0,
                    shallow,
                    s,
                    maxiter,
                );
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                let mut deep_state: State<CExp> = s.cast();
                // Extended range holds any magnitude: fold the rescaling exponent in.
                deep_state.dw = deep_state.dw.scale(FloatExp::ONE.ldexp(deep_state.dw_exp));
                deep_state.dw_exp = 0;
                *st = Hybrid::Deep(deep_state);
            }
        }
    }
}

/// Round to `f64`, flushing values that would be subnormal (or nearly so) to zero.
#[inline]
pub fn flush_f64(x: FloatExp) -> f64 {
    if x.exponent() < -1000 { 0.0 } else { x.to_f64() }
}

pub fn switch_norm<T: Real>() -> T {
    T::from_f64(SWITCH_NORM)
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::*;
    use std::arch::x86_64::*;

    /// One SIMD lane: a pixel's state, its current iteration limit, and
    /// whether it is in the first pass of two-pass interior detection.
    #[derive(Clone, Copy)]
    struct Lane {
        st: State<Complex<f64>>,
        pixel: usize,
        cap: u64,
        first_pass: bool,
    }

    /// Iterate `dcs` four pixels at a time. Lanes on the common path (a plain
    /// perturbation step with nothing to decide) advance together in AVX2
    /// registers. A lane with anything else to do (escape, rebase, a
    /// possible BLA step, interior checks, rescaling, its iteration limit)
    /// takes that iteration through the scalar loop, so every pixel gets
    /// exactly the operations the scalar FMA path would perform.
    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn batch<const INSIDE: bool>(
        reference: &Reference,
        p: &Phase<Complex<f64>>,
        dcs: &[Complex<f64>],
        maxiter: u64,
        out: &mut [PixelResult],
    ) {
        let periodic = reference.truncated.is_some();
        let two_pass = INSIDE && !periodic && maxiter > 2 * INTERIOR_START;
        let phase = |dc| Phase { bla: p.bla, dc, spacing: p.spacing, bailout2: p.bailout2, switch2: p.switch2 };
        let z_at = |m: usize| reference.z(m);
        let start = |pixel: usize| {
            if two_pass {
                Lane {
                    st: State::start(dcs[pixel], p.spacing, false, maxiter),
                    pixel,
                    cap: INTERIOR_START,
                    first_pass: true,
                }
            } else {
                Lane {
                    st: State::start(dcs[pixel], p.spacing, !periodic, maxiter),
                    pixel,
                    cap: maxiter,
                    first_pass: false,
                }
            }
        };
        // The second pass of two-pass interior detection restarts from z_1.
        let restart = |lane: &mut Lane| {
            let steps = lane.st.steps;
            lane.st = State { steps, ..State::start(dcs[lane.pixel], p.spacing, true, maxiter) };
            lane.cap = maxiter;
            lane.first_pass = false;
        };
        // One scalar iteration; true once the lane's pixel is finished.
        let step = |lane: &mut Lane, out: &mut [PixelResult]| -> bool {
            let ph = phase(dcs[lane.pixel]);
            match run::<_, false, INSIDE, false, false, false, true, true>(
                reference,
                z_at,
                |_| 0.0,
                &ph,
                &mut lane.st,
                lane.cap,
            ) {
                Exit::Yield => false,
                Exit::Stopped if lane.first_pass => {
                    restart(lane);
                    false
                }
                exit => {
                    out[lane.pixel] = finish(&lane.st, exit, 0);
                    true
                }
            }
        };
        // Run a lane to completion with the scalar loop.
        let complete = |lane: &mut Lane, out: &mut [PixelResult]| loop {
            let ph = phase(dcs[lane.pixel]);
            let exit = run::<_, false, INSIDE, false, false, false, true, false>(
                reference,
                z_at,
                |_| 0.0,
                &ph,
                &mut lane.st,
                lane.cap,
            );
            if lane.first_pass && matches!(exit, Exit::Stopped) {
                restart(lane);
                continue;
            }
            out[lane.pixel] = finish(&lane.st, exit, 0);
            break;
        };

        if dcs.len() < 4 {
            for pixel in 0..dcs.len() {
                complete(&mut start(pixel), out);
            }
            return;
        }
        let mut lanes = [start(0), start(1), start(2), start(3)];
        let mut next = 4;

        let orbit = reference.orbit().as_ptr() as *const f64;
        let mref = _mm256_set1_epi64x(reference.last_index() as i64);
        let bailout2 = _mm256_set1_pd(p.bailout2);
        let max_r2 = _mm256_set1_pd(p.bla.max_r2());
        let two = _mm256_set1_pd(2.0);
        let one = _mm256_set1_epi64x(1);
        let all = _mm256_set1_epi64x(-1);
        let ir2 = _mm256_set1_pd(INTERIOR_RADIUS2);
        let (big, small) = (_mm256_set1_pd(DW_BIG), _mm256_set1_pd(DW_SMALL));
        let nonperiodic = _mm256_castsi256_pd(_mm256_set1_epi64x(if periodic { 0 } else { -1 }));

        loop {
            // Registers from the lanes.
            let f = |g: &dyn Fn(&Lane) -> f64| _mm256_set_pd(g(&lanes[3]), g(&lanes[2]), g(&lanes[1]), g(&lanes[0]));
            let i =
                |g: &dyn Fn(&Lane) -> i64| _mm256_set_epi64x(g(&lanes[3]), g(&lanes[2]), g(&lanes[1]), g(&lanes[0]));
            let mask = |g: &dyn Fn(&Lane) -> bool| _mm256_castsi256_pd(i(&|l| if g(l) { -1 } else { 0 }));
            let (mut dzr, mut dzi) = (f(&|l| l.st.dz.re), f(&|l| l.st.dz.im));
            let (mut dwr, mut dwi) = (f(&|l| l.st.dw.re), f(&|l| l.st.dw.im));
            let (dcr, dci) = (f(&|l| dcs[l.pixel].re), f(&|l| dcs[l.pixel].im));
            let mut m = i(&|l| l.st.m as i64);
            let mut n = i(&|l| l.st.n as i64);
            let mut steps = i(&|l| l.st.steps as i64);
            let cap = i(&|l| l.cap as i64);
            let active = mask(&|l| l.st.dw_active);
            let exp0 = mask(&|l| l.st.dw_exp == 0);
            let exp_pos = mask(&|l| l.st.dw_exp > 0);

            let events = loop {
                let idx = _mm256_slli_epi64::<1>(m);
                // SAFETY: every lane's m is at most `last_index()`, so both
                // gathers read inside the orbit's `re`/`im` pairs.
                let (zr, zi) =
                    unsafe { (_mm256_i64gather_pd::<8>(orbit, idx), _mm256_i64gather_pd::<8>(orbit.add(1), idx)) };
                let xr = _mm256_add_pd(zr, dzr);
                let xi = _mm256_add_pd(zi, dzi);
                let zn = _mm256_fmadd_pd(xr, xr, _mm256_mul_pd(xi, xi));
                let dzn = _mm256_fmadd_pd(dzr, dzr, _mm256_mul_pd(dzi, dzi));
                let escape = _mm256_cmp_pd::<_CMP_GT_OQ>(zn, bailout2);
                let closer = _mm256_cmp_pd::<_CMP_LT_OQ>(zn, dzn);
                let at_end = _mm256_castsi256_pd(_mm256_cmpeq_epi64(m, mref));
                let at_cap = _mm256_castsi256_pd(_mm256_xor_si256(_mm256_cmpgt_epi64(cap, n), all));
                let m0 = _mm256_castsi256_pd(_mm256_cmpeq_epi64(m, _mm256_setzero_si256()));
                let bla = _mm256_andnot_pd(m0, _mm256_cmp_pd::<_CMP_LT_OQ>(dzn, max_r2));
                let mut ev =
                    _mm256_or_pd(_mm256_or_pd(escape, closer), _mm256_or_pd(at_end, _mm256_or_pd(at_cap, bla)));
                if INSIDE {
                    let dwn = _mm256_add_pd(_mm256_mul_pd(dwr, dwr), _mm256_mul_pd(dwi, dwi));
                    let inside = _mm256_and_pd(_mm256_and_pd(exp0, nonperiodic), _mm256_cmp_pd::<_CMP_LT_OQ>(dwn, ir2));
                    let rescale = _mm256_or_pd(
                        _mm256_cmp_pd::<_CMP_GT_OQ>(dwn, big),
                        _mm256_and_pd(exp_pos, _mm256_cmp_pd::<_CMP_LT_OQ>(dwn, small)),
                    );
                    ev = _mm256_or_pd(ev, _mm256_and_pd(active, _mm256_or_pd(inside, rescale)));
                }
                let bits = _mm256_movemask_pd(ev);
                if bits != 0 {
                    break bits;
                }
                if INSIDE {
                    // dw = (2 z) dw, an unfused complex product as in the scalar loop.
                    let z2r = _mm256_mul_pd(xr, two);
                    let z2i = _mm256_mul_pd(xi, two);
                    let ndwr = _mm256_sub_pd(_mm256_mul_pd(z2r, dwr), _mm256_mul_pd(z2i, dwi));
                    let ndwi = _mm256_add_pd(_mm256_mul_pd(z2r, dwi), _mm256_mul_pd(z2i, dwr));
                    dwr = _mm256_blendv_pd(dwr, ndwr, active);
                    dwi = _mm256_blendv_pd(dwi, ndwi, active);
                }
                // dz = (2 Z_m + dz) dz + dc, fused as in the scalar FMA path.
                let tr = _mm256_add_pd(_mm256_mul_pd(zr, two), dzr);
                let ti = _mm256_add_pd(_mm256_mul_pd(zi, two), dzi);
                let ndzr = _mm256_fmadd_pd(tr, dzr, _mm256_fnmadd_pd(ti, dzi, dcr));
                let ndzi = _mm256_fmadd_pd(tr, dzi, _mm256_fmadd_pd(ti, dzr, dci));
                dzr = ndzr;
                dzi = ndzi;
                m = _mm256_add_epi64(m, one);
                n = _mm256_add_epi64(n, one);
                steps = _mm256_add_epi64(steps, one);
            };

            // Back to the lanes.
            let mut buf = [[0f64; 4]; 4];
            let mut ibuf = [[0i64; 4]; 3];
            // SAFETY: each store writes four lanes into a four-element local array.
            unsafe {
                _mm256_storeu_pd(buf[0].as_mut_ptr(), dzr);
                _mm256_storeu_pd(buf[1].as_mut_ptr(), dzi);
                _mm256_storeu_pd(buf[2].as_mut_ptr(), dwr);
                _mm256_storeu_pd(buf[3].as_mut_ptr(), dwi);
                _mm256_storeu_si256(ibuf[0].as_mut_ptr() as *mut __m256i, m);
                _mm256_storeu_si256(ibuf[1].as_mut_ptr() as *mut __m256i, n);
                _mm256_storeu_si256(ibuf[2].as_mut_ptr() as *mut __m256i, steps);
            }
            for (k, lane) in lanes.iter_mut().enumerate() {
                lane.st.dz = Complex::new(buf[0][k], buf[1][k]);
                lane.st.dw = Complex::new(buf[2][k], buf[3][k]);
                lane.st.m = ibuf[0][k] as usize;
                lane.st.n = ibuf[1][k] as u64;
                lane.st.steps = ibuf[2][k] as u64;
            }
            // Lanes with an event take this iteration through the scalar loop.
            let mut live = [true; 4];
            for (k, lane) in lanes.iter_mut().enumerate() {
                if events & (1 << k) != 0 && step(lane, out) {
                    if next < dcs.len() {
                        *lane = start(next);
                        next += 1;
                    } else {
                        live[k] = false;
                    }
                }
            }
            if live.contains(&false) {
                // No pixels left to refill with: finish the rest one by one.
                for (lane, live) in lanes.iter_mut().zip(live) {
                    if live {
                        complete(lane, out);
                    }
                }
                return;
            }
        }
    }
}
