//! High-precision reference orbits.

use crate::Progress;
use crate::fixed::{self, Fixed};
use crate::floatexp::FloatExp;
use crate::num::{CExp, Complex, Cplx};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

/// Below this magnitude an orbit value is flushed to zero in the `f64` array
/// (keeping subnormals out of the hot loops) and kept exactly in `tiny`.
pub const TINY: f64 = f64::from_bits((1023 - 960) << 52); // 2^-960

/// A reference orbit may stop at `n` once `|Z_n| / |dZ_n/dC|` is below the
/// pixel spacing times `2^STOP_MARGIN_BITS`; see [`Reference::compute`].
pub const STOP_MARGIN_BITS: i32 = -8;

/// Escape radius for the reference orbit. Pixels keep iterating past the
/// reference's escape thanks to rebasing, so this only needs to be >= 2.
const REF_BAILOUT2: f64 = 4.0;

/// An orbit value prepared for the extended-range phase: the value itself
/// and its squared magnitude in `f64` (zero for values below [`TINY`]).
#[derive(Clone, Copy, Debug)]
pub struct DeepZ {
    pub z: CExp,
    pub norm: f64,
}

/// The orbit `Z_{n+1} = Z_n^2 + C` of a reference point `C`.
pub struct Reference {
    pub re: Fixed,
    pub im: Fixed,
    /// `Z_0 ..= Z_len()`, rounded to `f64`; values below [`TINY`] are zero.
    orbit: Vec<Complex<f64>>,
    /// Extended-range values for every index whose `f64` entry is exactly zero.
    tiny: Vec<(usize, Complex<FloatExp>)>,
    /// Whether the reference escaped (otherwise it reached `maxiter` or is periodic).
    pub escaped: bool,
    /// Set when the orbit stopped because it returned to zero (see
    /// [`Reference::compute`]): holds `|Z_len| / |dZ_len/dC|`. The reference
    /// then serves any `maxiter` for views whose `spacing * 2^STOP_MARGIN_BITS`
    /// exceeds it.
    pub truncated: Option<FloatExp>,
    /// The iteration limit it was computed (or extended) to.
    pub maxiter: u64,
    /// Lowest period of a minibrot nucleus within the probe radius given to
    /// [`Reference::compute`] (ball method: the first `n` with
    /// `|Z_n| < |dZ_n/dC| * radius`), if the orbit got that far.
    pub nearby_period: Option<u64>,
    /// State for continuing the orbit; `None` once it escaped or stopped.
    state: Option<Continuation>,
    /// The orbit as [`DeepZ`] values, built on first use by deep zooms.
    deep: OnceLock<Vec<DeepZ>>,
}

struct Continuation {
    z: FixedOrbit,
    der: Complex<FloatExp>,
    stop2: FloatExp,
    probe2: FloatExp,
}

impl Reference {
    /// Compute the orbit of `re + i im` until it escapes, reaches `maxiter`,
    /// or returns to (numerically) zero for images with pixel spacing
    /// `spacing` (pass zero to disable). Pixels rebase onto `Z_0` at the end
    /// of the orbit with `z = Z_n + dz`; once `|Z_n| / |dZ_n/dC|` is far below
    /// the spacing, `|Z_n|` is far below every pixel's `|dz|`, so the rebase
    /// adds no error beyond perturbation's own and the orbit can stop. This
    /// happens at the period when the reference sits on a minibrot nucleus.
    ///
    /// (Stopping whenever the rounding of `Z_n` is merely small next to the
    /// pixel spacing is *not* safe: near minibrots the repeated rebasing
    /// errors grow faster than the differences between neighbouring pixels.)
    ///
    /// Along the way it records the lowest period of a minibrot within
    /// `probe_radius` (zero to disable) in [`Reference::nearby_period`].
    ///
    /// Returns `None` if cancelled through `progress`.
    pub fn compute(
        re: &Fixed,
        im: &Fixed,
        maxiter: u64,
        spacing: FloatExp,
        probe_radius: FloatExp,
        progress: &Progress,
    ) -> Option<Reference> {
        let square = |x: FloatExp| x * x;
        let mut r = Reference {
            re: re.clone(),
            im: im.clone(),
            orbit: Vec::with_capacity((maxiter.min(1 << 24) + 1) as usize),
            tiny: Vec::new(),
            escaped: false,
            truncated: None,
            maxiter: 0,
            nearby_period: None,
            deep: OnceLock::new(),
            state: Some(Continuation {
                z: FixedOrbit::new(re, im),
                der: Complex::ZERO,
                stop2: square(spacing.ldexp(STOP_MARGIN_BITS)),
                probe2: square(probe_radius),
            }),
        };
        r.extend(maxiter, progress).then_some(r)
    }

    /// Whether the orbit is complete for an iteration limit of `maxiter`.
    pub fn covers(&self, maxiter: u64) -> bool {
        self.state.is_none() || self.maxiter >= maxiter
    }

    /// Continue the orbit up to `maxiter` (unless it escaped or stopped).
    /// Returns `false` if cancelled through `progress`; the orbit then stays
    /// valid up to `self.maxiter` and can be extended again later.
    pub fn extend(&mut self, maxiter: u64, progress: &Progress) -> bool {
        if self.covers(maxiter) && !self.orbit.is_empty() {
            return true;
        }
        let Some(mut st) = self.state.take() else { return true };
        self.deep = OnceLock::new();
        progress.set_total(maxiter);
        // Index of the orbit value about to be stored.
        let mut i = self.orbit.len() as u64;
        if i > 0 {
            // The stored state is Z_{i-1}; advance it to Z_i.
            st.z.step();
        }
        loop {
            let zfe = st.z.z_fe();
            let (fr, fi) = (zfe.re.to_f64(), zfe.im.to_f64());
            if fr.abs().max(fi.abs()) < TINY {
                self.tiny.push((self.orbit.len(), zfe));
                self.orbit.push(Complex::new(0.0, 0.0));
            } else {
                // Flush a subnormal component; it is negligible next to the other one.
                let flush = |v: f64| if v.abs() < TINY { 0.0 } else { v };
                self.orbit.push(Complex::new(flush(fr), flush(fi)));
            }
            if fr * fr + fi * fi > REF_BAILOUT2 {
                self.escaped = true;
                break;
            }
            if i > 0 {
                let (z2, d2) = (zfe.norm(), st.der.norm());
                if self.nearby_period.is_none() && z2 < d2 * st.probe2 {
                    self.nearby_period = Some(i);
                }
                if z2 < d2 * st.stop2 {
                    self.truncated = Some(zfe.abs() / st.der.abs());
                    break;
                }
            }
            st.der = zfe.mul2() * st.der + Complex::ONE;
            if i >= maxiter {
                self.maxiter = i;
                self.state = Some(st);
                progress.done.store(i, Ordering::Relaxed);
                return true;
            }
            if i & 0xfff == 0 {
                if progress.cancelled() {
                    self.maxiter = i;
                    self.state = Some(st);
                    return false;
                }
                progress.done.store(i, Ordering::Relaxed);
            }
            st.z.step();
            i += 1;
        }
        // Escaped or stopped: the orbit is final.
        progress.done.store(i, Ordering::Relaxed);
        self.maxiter = self.maxiter.max(maxiter).max(i);
        self.orbit.shrink_to_fit();
        true
    }

    /// Index of the last stored orbit value; pixels rebase when they reach it.
    #[inline]
    pub fn last_index(&self) -> usize {
        self.orbit.len() - 1
    }

    /// The whole orbit in `f64` (see [`Reference::z`]).
    pub fn orbit(&self) -> &[Complex<f64>] {
        &self.orbit
    }

    #[inline(always)]
    pub fn z(&self, m: usize) -> Complex<f64> {
        self.orbit[m]
    }

    #[inline(always)]
    pub fn z_fe(&self, m: usize) -> Complex<FloatExp> {
        let z = self.orbit[m];
        if z.re == 0.0 && z.im == 0.0 {
            match self.tiny.binary_search_by_key(&m, |t| t.0) {
                Ok(i) => self.tiny[i].1,
                Err(_) => Complex::ZERO,
            }
        } else {
            z.to_fe()
        }
    }

    #[inline(always)]
    pub fn z_cexp(&self, m: usize) -> CExp {
        let z = self.orbit[m];
        if z.re == 0.0 && z.im == 0.0 { CExp::from_cfe(self.z_fe(m)) } else { CExp::new(z.re, z.im, 0) }
    }

    /// The orbit prepared for the extended-range phase (built once, on first use).
    pub fn deep_orbit(&self) -> &[DeepZ] {
        self.deep.get_or_init(|| {
            (0..self.orbit.len()).map(|m| DeepZ { z: self.z_cexp(m), norm: self.orbit[m].norm() }).collect()
        })
    }

    pub fn memory_bytes(&self) -> usize {
        self.orbit.capacity() * size_of::<Complex<f64>>()
            + self.tiny.capacity() * size_of::<(usize, Complex<FloatExp>)>()
    }
}

/// In-place fixed-point iteration of `z -> z^2 + c`, starting from `z = 0`.
pub struct FixedOrbit {
    cr: Vec<u64>,
    ci: Vec<u64>,
    x: Vec<u64>,
    y: Vec<u64>,
    ax: Vec<u64>,
    ay: Vec<u64>,
    s: Vec<u64>,
    x2: Vec<u64>,
    y2: Vec<u64>,
    s2: Vec<u64>,
    prod: Vec<u64>,
}

impl FixedOrbit {
    pub fn new(re: &Fixed, im: &Fixed) -> Self {
        let n = re.limbs();
        assert_eq!(n, im.limbs());
        let v = || vec![0u64; n];
        FixedOrbit {
            cr: re.raw().to_vec(),
            ci: im.raw().to_vec(),
            x: v(),
            y: v(),
            ax: v(),
            ay: v(),
            s: v(),
            x2: v(),
            y2: v(),
            s2: v(),
            prod: vec![0u64; 2 * n],
        }
    }

    #[inline]
    pub fn z_fe(&mut self) -> Complex<FloatExp> {
        Complex::new(fixed::limbs_to_floatexp(&self.x, &mut self.ax), fixed::limbs_to_floatexp(&self.y, &mut self.ay))
    }

    /// `x' = x^2 - y^2 + cr ; y' = (x + y)^2 - x^2 - y^2 + ci`
    #[inline]
    pub fn step(&mut self) {
        fixed::abs_into(&mut self.ax, &self.x);
        fixed::abs_into(&mut self.ay, &self.y);
        fixed::add_into(&mut self.s, &self.x, &self.y);
        if fixed::is_neg(&self.s) {
            fixed::negate(&mut self.s);
        }
        fixed::sqr_mag(&self.ax, &mut self.x2, &mut self.prod);
        fixed::sqr_mag(&self.ay, &mut self.y2, &mut self.prod);
        fixed::sqr_mag(&self.s, &mut self.s2, &mut self.prod);
        self.x.copy_from_slice(&self.x2);
        fixed::sub_assign(&mut self.x, &self.y2);
        fixed::add_assign(&mut self.x, &self.cr);
        self.y.copy_from_slice(&self.s2);
        fixed::sub_assign(&mut self.y, &self.x2);
        fixed::sub_assign(&mut self.y, &self.y2);
        fixed::add_assign(&mut self.y, &self.ci);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_constant() {
        assert_eq!(TINY, 2f64.powi(-960));
    }

    #[test]
    fn matches_f64_orbit() {
        let (cr, ci) = (-0.75, 0.1);
        let r = Reference::compute(
            &Fixed::from_f64(cr, 4),
            &Fixed::from_f64(ci, 4),
            1000,
            FloatExp::ZERO,
            FloatExp::ZERO,
            &Progress::default(),
        )
        .unwrap();
        let (mut x, mut y) = (0.0f64, 0.0f64);
        for m in 0..=r.last_index() {
            let z = r.z(m);
            assert!((z.re - x).abs() < 1e-9 && (z.im - y).abs() < 1e-9, "m={m}");
            (x, y) = (x * x - y * y + cr, 2.0 * x * y + ci);
        }
        assert!(r.escaped);
        assert!(r.z_fe(0).re.is_zero());
    }

    #[test]
    fn interior_runs_to_maxiter() {
        let r = Reference::compute(
            &Fixed::from_f64(-0.1, 3),
            &Fixed::from_f64(0.1, 3),
            500,
            FloatExp::ZERO,
            FloatExp::ZERO,
            &Progress::default(),
        )
        .unwrap();
        assert!(!r.escaped);
        assert_eq!(r.last_index(), 500);
        assert!(r.covers(500) && !r.covers(501));
    }

    #[test]
    fn extending_matches_computing_at_once() {
        let p = Progress::default();
        let (re, im) = (Fixed::from_f64(-0.1, 3), Fixed::from_f64(0.1, 3));
        let mut a = Reference::compute(&re, &im, 300, FloatExp::ZERO, FloatExp::ZERO, &p).unwrap();
        assert!(a.extend(800, &p));
        let b = Reference::compute(&re, &im, 800, FloatExp::ZERO, FloatExp::ZERO, &p).unwrap();
        assert_eq!(a.last_index(), 800);
        assert_eq!(a.last_index(), b.last_index());
        assert!((0..=800).all(|m| a.z(m) == b.z(m)));
    }

    #[test]
    fn nucleus_is_periodic() {
        let n = 4;
        let p = Progress::default();
        let (cr, ci) = crate::newton::find_nucleus(&Fixed::from_f64(-1.75, n), &Fixed::zero(n), 3, 50, &p).unwrap();
        let r = Reference::compute(&cr, &ci, 1000, FloatExp::from(1e-6), FloatExp::from(0.01), &p).unwrap();
        assert_eq!(r.last_index(), 3);
        assert!(r.truncated.is_some() && !r.escaped);
        assert_eq!(r.nearby_period, Some(3));
    }
}
