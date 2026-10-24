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
//! a cycle, as in Fraktaler 3).

use crate::bla::BlaTable;
use crate::floatexp::FloatExp;
use crate::num::{CExp, Complex, Cplx, Real};
use crate::reference::Reference;

/// `|dz|^2` above which a deep pixel switches to `f64` (`|dz| > 2^-480`). At
/// that size `dz^2` and `2 Z dz` stay normal doubles, and a dropped `dc` below
/// 2^-1000 is negligible next to them.
const SWITCH_NORM: f64 = f64::from_bits((1023 - 960) << 52);

/// `|dz_n / dz_k|^2` below which a pixel counts as interior (Fraktaler 3's
/// default inscape radius of 1/1024, there with `k = 1`).
pub const INTERIOR_RADIUS2: f64 = 1.0 / (1024.0 * 1024.0);

/// With high iteration limits, interior detection starts tracking `dz_n /
/// dz_k` only at the first rebase after iteration `INTERIOR_START`, so most
/// exterior pixels escape without paying for the extra derivative. The start
/// must be a rebase onto `Z_0` (where `z` is near zero, like `z_0`): starting
/// mid-period lets exterior orbits dip below the threshold as they pass zero.
const INTERIOR_START: u64 = 4096;

/// Rescaling of `dz_n/dz_1` in `f64`, which can exceed the range of doubles.
const DW_BIG: f64 = f64::from_bits((1023 + 600) << 52);
const DW_SMALL: f64 = f64::from_bits((1023 - 600) << 52);
const DW_STEP: i32 = 300;

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
    /// `dz_n/dz_k`, times `2^dw_exp`, once `dw_active`.
    dw: C,
    dw_exp: i32,
    dw_active: bool,
    m: usize,
    n: u64,
    steps: u64,
}

impl<C: Cplx> State<C> {
    /// The state after the first iteration: `z_1 = c`, so `dz_1 = dc`.
    fn start(dc: C, spacing: C::R, maxiter: u64) -> Self {
        let dw_active = maxiter <= 2 * INTERIOR_START;
        State { dz: dc, der: C::from_real(spacing), dw: C::ONE, dw_exp: 0, dw_active, m: 1, n: 1, steps: 0 }
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
#[inline(always)]
fn run<C: Cplx, const DE: bool, const INSIDE: bool, const UP: bool, const DOWN: bool>(
    reference: &Reference,
    z_at: impl Fn(usize) -> C,
    p: &Phase<C>,
    st: &mut State<C>,
    maxiter: u64,
) -> Exit {
    let mref = reference.last_index();
    let periodic = reference.truncated.is_some();
    let State { mut dz, mut der, mut dw, mut dw_exp, mut dw_active, mut m, mut n, mut steps } = *st;
    let ir2 = C::R::from_f64(INTERIOR_RADIUS2);
    let (dw_big, dw_small) = (C::R::from_f64(DW_BIG), C::R::from_f64(DW_SMALL));
    let mut zm = z_at(m);
    // Set on activation: measure from the iterate *after* the near-zero one
    // (like starting at z_1 rather than z_0), skipping its tiny factor 2z.
    let mut dw_fresh = false;
    let exit = loop {
        let mut z = zm + dz;
        let zn = z.norm();
        let mut dzn = dz.norm();
        if zn > p.bailout2 {
            break Exit::Escaped(z.to_c64());
        }
        if zn < dzn || m == mref {
            if INSIDE && !dw_active && n >= INTERIOR_START && (zn < dzn || periodic) {
                dw_active = true;
                dw_fresh = true;
                dw = C::ONE;
                dw_exp = 0;
            }
            dz = z;
            dzn = zn;
            m = 0;
            zm = C::ZERO;
            z = dz;
        }
        if n >= maxiter {
            break Exit::Stopped;
        }
        if (UP && dzn > p.switch2) || (DOWN && dzn < p.switch2) {
            break Exit::Switch;
        }
        if INSIDE && dw_active {
            let dwn = dw.norm();
            if dw_exp == 0 && dwn < ir2 {
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
        if let Some(b) = p.bla.lookup(m, dzn, maxiter - n) {
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
            let z2 = z.mul2();
            if DE {
                der = z2 * der + C::from_real(p.spacing);
            }
            if INSIDE && dw_active {
                if dw_fresh {
                    dw_fresh = false;
                } else {
                    dw = z2 * dw;
                }
            }
            dz = (zm.mul2() + dz) * dz + p.dc;
            m += 1;
            n += 1;
        }
        zm = z_at(m);
    };
    *st = State { dz, der, dw, dw_exp, dw_active, m, n, steps };
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
    let mut st = State::start(p.dc, p.spacing, maxiter);
    let exit = run::<_, DE, INSIDE, false, false>(reference, |m| reference.z(m), p, &mut st, maxiter);
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
    dispatch!(extras, iterate_deep_impl::<_, _>(reference, deep, shallow, maxiter))
}

#[inline(always)]
fn iterate_deep_impl<const DE: bool, const INSIDE: bool>(
    reference: &Reference,
    deep: &Phase<CExp>,
    shallow: &Phase<Complex<f64>>,
    maxiter: u64,
) -> PixelResult {
    let mut st = Hybrid::Deep(State::start(deep.dc, deep.spacing, maxiter));
    let mut deep_steps = 0;
    let exit = run_hybrid::<DE, INSIDE>(reference, deep, shallow, &mut st, maxiter, &mut deep_steps);
    st.finish(exit, deep_steps)
}

/// A deep pixel's state in whichever representation it currently uses.
enum Hybrid {
    Deep(State<CExp>),
    Shallow(State<Complex<f64>>),
}

impl Hybrid {
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
                let exit = run::<_, DE, INSIDE, true, false>(reference, |m| reference.z_cexp(m), deep, s, maxiter);
                *deep_steps += s.steps - before;
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                *st = Hybrid::Shallow(s.cast());
            }
            Hybrid::Shallow(s) => {
                let exit = run::<_, DE, INSIDE, false, true>(reference, |m| reference.z(m), shallow, s, maxiter);
                if !matches!(exit, Exit::Switch) {
                    return exit;
                }
                *st = Hybrid::Deep(s.cast());
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
