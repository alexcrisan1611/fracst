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
use std::sync::OnceLock;

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

/// Dev-only experiment switch for comparing extended-range variants:
/// `FRACST_DEEP=baseline` (orbit converted per step), `orbit` (precomputed
/// orbit), `fast` (plus the exact fast path and lean interior detection) or
/// `top` (plus longest-first BLA lookups; default).
fn deep_variant() -> u8 {
    static VARIANT: OnceLock<u8> = OnceLock::new();
    *VARIANT.get_or_init(|| match std::env::var("FRACST_DEEP").as_deref() {
        Ok("baseline") => 0,
        Ok("orbit") => 1,
        Ok("fast") => 2,
        _ => 3,
    })
}

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
}

/// Iterate until escape, `maxiter`, interior, or (if `UP`/`DOWN`) `|dz|^2`
/// crossing `switch2`.
///
/// `DEEP` selects extended-range optimisations (0 = none; see
/// [`deep_variant`]). With `DEEP >= 2`, `zn_at(m)` gives `|Z_m|^2` in
/// `f64`. While `|dz|^2 <= switch2` and `Z_m` is neither tiny nor near the
/// bailout, `Z_m + dz` can neither escape nor rebase, so those tests (and
/// the extended-range sum and norm behind them) are skipped. The results
/// are the same either way.
#[inline(always)]
fn run<C: Cplx, const DE: bool, const INSIDE: bool, const UP: bool, const DOWN: bool, const DEEP: u8>(
    reference: &Reference,
    z_at: impl Fn(usize) -> C,
    zn_at: impl Fn(usize) -> f64,
    p: &Phase<C>,
    st: &mut State<C>,
    maxiter: u64,
) -> Exit {
    let mref = reference.last_index();
    let periodic = reference.truncated.is_some();
    let fast_path = DEEP >= 2;
    let State { mut dz, mut der, mut dw, mut dw_exp, mut dw_active, mut dw_below, dw_start, mut m, mut n, mut steps } =
        *st;
    let ir2 = C::R::from_f64(INTERIOR_RADIUS2);
    let (dw_big, dw_small) = (C::R::from_f64(DW_BIG), C::R::from_f64(DW_SMALL));
    let mut zm = z_at(m);
    // Below a quarter of the bailout, |Z_m + dz| stays inside the escape radius.
    let zn_escape = if fast_path { p.bailout2.to_fe().to_f64() * 0.25 } else { 0.0 };
    let exit = loop {
        let mut dzn = dz.norm();
        let fast = fast_path && m != mref && !(dzn > p.switch2) && {
            let zmn = zn_at(m);
            zmn >= ORDINARY_NORM && zmn <= zn_escape
        };
        let mut z = zm;
        if !fast {
            z = zm + dz;
            let zn = z.norm();
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
        let bla = if DEEP >= 3 { p.bla.lookup_longest(m, dzn, maxiter - n) } else { p.bla.lookup(m, dzn, maxiter - n) };
        if let Some(b) = bla {
            dz = b.a * dz + b.b * p.dc;
            if DE {
                der = b.a * der + b.b.scale(p.spacing);
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
                    der = z2 * der + C::from_real(p.spacing);
                }
                if INSIDE && dw_active {
                    dw = z2 * dw;
                }
            }
            dz = (zm.mul2() + dz) * dz + p.dc;
            m += 1;
            n += 1;
        }
        zm = z_at(m);
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
pub fn iterate_f64(reference: &Reference, p: &Phase<Complex<f64>>, maxiter: u64, extras: Extras) -> PixelResult {
    dispatch!(extras, iterate_f64_impl::<_, _>(reference, p, maxiter))
}

#[inline(always)]
fn iterate_f64_impl<const DE: bool, const INSIDE: bool>(
    reference: &Reference,
    p: &Phase<Complex<f64>>,
    maxiter: u64,
) -> PixelResult {
    let z_at = |m| reference.z(m);
    let periodic = reference.truncated.is_some();
    let mut steps = 0;
    if INSIDE && !periodic && maxiter > 2 * INTERIOR_START {
        let mut st = State::start(p.dc, p.spacing, false, maxiter);
        let exit = run::<_, DE, false, false, false, 0>(reference, z_at, |_| 0.0, p, &mut st, INTERIOR_START);
        if !matches!(exit, Exit::Stopped) {
            return finish(&st, exit, 0);
        }
        steps = st.steps;
    }
    let mut st = State::start(p.dc, p.spacing, !periodic, maxiter);
    st.steps = steps;
    let exit = run::<_, DE, INSIDE, false, false, 0>(reference, z_at, |_| 0.0, p, &mut st, maxiter);
    finish(&st, exit, 0)
}

/// Iterate one pixel of a deep zoom: [`CExp`] while `|dz|` is tiny, `f64` otherwise.
pub fn iterate_deep(
    reference: &Reference,
    deep: &Phase<CExp>,
    shallow: &Phase<Complex<f64>>,
    maxiter: u64,
    extras: Extras,
) -> PixelResult {
    match deep_variant() {
        0 => dispatch!(extras, iterate_deep_impl::<_, _, 0>(reference, deep, shallow, maxiter)),
        1 => dispatch!(extras, iterate_deep_impl::<_, _, 1>(reference, deep, shallow, maxiter)),
        2 => dispatch!(extras, iterate_deep_impl::<_, _, 2>(reference, deep, shallow, maxiter)),
        _ => dispatch!(extras, iterate_deep_impl::<_, _, 3>(reference, deep, shallow, maxiter)),
    }
}

#[inline(always)]
fn iterate_deep_impl<const DE: bool, const INSIDE: bool, const VARIANT: u8>(
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
        let exit = run_hybrid::<DE, false, VARIANT>(reference, deep, shallow, &mut st, INTERIOR_START, &mut deep_steps);
        if !matches!(exit, Exit::Stopped) {
            return st.finish(exit, deep_steps);
        }
        steps = st.steps();
    }
    let mut st = Hybrid::Deep(State { steps, ..State::start(deep.dc, deep.spacing, !periodic, maxiter) });
    let exit = run_hybrid::<DE, INSIDE, VARIANT>(reference, deep, shallow, &mut st, maxiter, &mut deep_steps);
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
fn run_hybrid<const DE: bool, const INSIDE: bool, const VARIANT: u8>(
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
                let exit = match VARIANT {
                    0 => run::<_, DE, INSIDE, true, false, 0>(
                        reference,
                        |m| reference.z_cexp(m),
                        |_| 0.0,
                        deep,
                        s,
                        maxiter,
                    ),
                    1 => {
                        let orbit = reference.deep_orbit();
                        run::<_, DE, INSIDE, true, false, 1>(reference, |m| orbit[m].z, |_| 0.0, deep, s, maxiter)
                    }
                    2 => {
                        let orbit = reference.deep_orbit();
                        run::<_, DE, INSIDE, true, false, 2>(
                            reference,
                            |m| orbit[m].z,
                            |m| orbit[m].norm,
                            deep,
                            s,
                            maxiter,
                        )
                    }
                    _ => {
                        let orbit = reference.deep_orbit();
                        run::<_, DE, INSIDE, true, false, 3>(
                            reference,
                            |m| orbit[m].z,
                            |m| orbit[m].norm,
                            deep,
                            s,
                            maxiter,
                        )
                    }
                };
                *deep_steps += s.steps - before;
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                *st = Hybrid::Shallow(s.cast());
            }
            Hybrid::Shallow(s) => {
                let exit =
                    run::<_, DE, INSIDE, false, true, 0>(reference, |m| reference.z(m), |_| 0.0, shallow, s, maxiter);
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                let mut deep_state: State<CExp> = s.cast();
                if VARIANT >= 2 {
                    // Extended range holds any magnitude: fold the rescaling exponent in.
                    deep_state.dw = deep_state.dw.scale(FloatExp::ONE.ldexp(deep_state.dw_exp));
                    deep_state.dw_exp = 0;
                }
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
