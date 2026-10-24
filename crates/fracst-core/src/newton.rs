//! Minibrot finding ("Newton zooming"): period detection with the ball
//! method, Newton's method for the nucleus, and an atom size estimate.

use crate::Progress;
use crate::fixed::{Fixed, limbs_for_bits};
use crate::floatexp::FloatExp;
use crate::num::Complex;
use crate::reference::FixedOrbit;

type C = Complex<FloatExp>;

/// A minibrot: nucleus position, period and approximate radius.
#[derive(Clone, Debug)]
pub struct Nucleus {
    pub re: Fixed,
    pub im: Fixed,
    pub period: u64,
    pub size: FloatExp,
}

/// Lowest period of a nucleus within `radius` of `c`, by iterating a disk
/// (centre plus linearised radius) until it first surrounds zero.
pub fn find_period(re: &Fixed, im: &Fixed, radius: FloatExp, maxperiod: u64, progress: &Progress) -> Option<u64> {
    let mut z = FixedOrbit::new(re, im);
    let mut der = C::ZERO;
    let r2 = radius * radius;
    let four = FloatExp::from(4.0);
    for n in 1..=maxperiod {
        der = z.z_fe().mul2() * der + C::ONE;
        z.step();
        let zn2 = z.z_fe().norm();
        if zn2 < der.norm() * r2 {
            return Some(n);
        }
        if zn2 > four || (n & 0xfff == 0 && progress.cancelled()) {
            return None;
        }
    }
    None
}

/// Refine a nucleus of the given period with Newton's method at the
/// precision of `re`/`im`. Returns `None` if it does not converge.
pub fn find_nucleus(
    re: &Fixed,
    im: &Fixed,
    period: u64,
    max_steps: usize,
    progress: &Progress,
) -> Option<(Fixed, Fixed)> {
    let n = re.limbs();
    let (mut cr, mut ci) = (re.clone(), im.clone());
    // Converged once a step is within ~20 bits of the precision floor.
    let floor = FloatExp::exp2(20.0 - re.precision_bits() as f64);
    for _ in 0..max_steps {
        let mut z = FixedOrbit::new(&cr, &ci);
        let mut der = C::ZERO;
        for i in 0..period {
            der = z.z_fe().mul2() * der + C::ONE;
            z.step();
            if i & 0xfff == 0 && progress.cancelled() {
                return None;
            }
        }
        let delta = -(z.z_fe() / der);
        if !delta.is_finite() || delta.abs() > FloatExp::from(4.0) {
            return None;
        }
        cr = cr.add(&Fixed::from_floatexp(delta.re, n));
        ci = ci.add(&Fixed::from_floatexp(delta.im, n));
        if delta.abs() < floor {
            return Some((cr, ci));
        }
    }
    None
}

/// Complex size estimate of the atom with this nucleus; its modulus is
/// roughly the minibrot's radius.
pub fn size(re: &Fixed, im: &Fixed, period: u64) -> C {
    let mut z = FixedOrbit::new(re, im);
    let (mut l, mut b) = (C::ONE, C::ONE);
    z.step();
    for _ in 1..period {
        l = z.z_fe().mul2() * l;
        b = b + C::ONE / l;
        z.step();
    }
    C::ONE / (b * l * l)
}

/// Find the lowest-period minibrot within `radius` of `c` and locate its
/// nucleus precisely enough to zoom all the way into it.
pub fn find_minibrot(re: &Fixed, im: &Fixed, radius: FloatExp, maxperiod: u64, progress: &Progress) -> Option<Nucleus> {
    let period = find_period(re, im, radius, maxperiod, progress)?;
    // Minibrots are usually much smaller than the region they were found in:
    // start with twice the view's precision and grow until the size settles.
    let view_bits = (-radius.log2()).max(0.0) as u64;
    let mut limbs = limbs_for_bits(2 * view_bits + 64).max(re.limbs());
    let (mut cr, mut ci) = (re.with_limbs(limbs), im.with_limbs(limbs));
    for _ in 0..6 {
        (cr, ci) = find_nucleus(&cr, &ci, period, 200, progress)?;
        let s = size(&cr, &ci, period).abs();
        let needed = limbs_for_bits((-s.log2()).max(0.0) as u64 + 64);
        if needed <= limbs {
            return Some(Nucleus { re: cr, im: ci, period, size: s });
        }
        limbs = needed + 1;
        (cr, ci) = (cr.with_limbs(limbs), ci.with_limbs(limbs));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_three_nucleus() {
        let n = 4;
        let p = Progress::default();
        let (re, im) = (Fixed::from_f64(-1.75, n), Fixed::zero(n));
        assert_eq!(find_period(&re, &im, FloatExp::from(0.01), 100, &p), Some(3));
        let (cr, ci) = find_nucleus(&re, &im, 3, 50, &p).unwrap();
        assert!((cr.to_f64() + 1.754877666246693).abs() < 1e-14 && ci.to_f64().abs() < 1e-14);
        // The period-3 minibrot on the real axis is about 0.019 across... radius ~ 0.0094ish.
        let s = size(&cr, &ci, 3).abs().to_f64();
        assert!(s > 0.005 && s < 0.03, "{s}");
    }
}
