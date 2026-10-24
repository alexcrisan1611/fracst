//! Extended-range floating point: an `f64` mantissa with a separate `i32` exponent.
//!
//! Used wherever values leave the range of `f64` (zooms past ~1e300): pixel
//! deltas, BLA coefficients and validity radii.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// `m * 2^e` with `m == 0` or `1 <= |m| < 2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatExp {
    m: f64,
    e: i32,
}

const ZERO_EXP: i32 = i32::MIN / 4;
const TWO_POW_64: f64 = 18446744073709551616.0;

#[inline(always)]
fn pow2(k: i32) -> f64 {
    debug_assert!((-1022..=1023).contains(&k));
    f64::from_bits(((k + 1023) as u64) << 52)
}

impl FloatExp {
    pub const ZERO: Self = FloatExp { m: 0.0, e: ZERO_EXP };
    pub const ONE: Self = FloatExp { m: 1.0, e: 0 };

    #[inline(always)]
    pub fn new(m: f64, e: i32) -> Self {
        let bits = m.to_bits();
        let biased = ((bits >> 52) & 0x7ff) as i32;
        if biased == 0 {
            if m == 0.0 {
                return Self::ZERO;
            }
            return Self::new(m * TWO_POW_64, e - 64);
        }
        FloatExp { m: f64::from_bits((bits & 0x800f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000), e: e + (biased - 1023) }
    }

    /// Mantissa in `[1, 2)` (or 0) and binary exponent.
    #[inline]
    pub fn parts(self) -> (f64, i32) {
        (self.m, self.e)
    }

    #[inline]
    pub fn exponent(self) -> i32 {
        self.e
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.m == 0.0
    }

    #[inline]
    pub fn to_f64(self) -> f64 {
        if self.e >= -1022 {
            if self.e > 1023 {
                return self.m * f64::INFINITY;
            }
            self.m * pow2(self.e)
        } else if self.e < -1100 {
            0.0
        } else {
            self.m * pow2(self.e + 128) * pow2(-128)
        }
    }

    #[inline]
    pub fn abs(self) -> Self {
        FloatExp { m: self.m.abs(), e: self.e }
    }

    #[inline]
    pub fn mul2(self) -> Self {
        if self.m == 0.0 { self } else { FloatExp { m: self.m, e: self.e + 1 } }
    }

    /// Multiply by `2^k`.
    #[inline]
    pub fn ldexp(self, k: i32) -> Self {
        if self.m == 0.0 { self } else { FloatExp { m: self.m, e: self.e + k } }
    }

    pub fn sqrt(self) -> Self {
        if self.m <= 0.0 {
            return Self::ZERO;
        }
        if self.e & 1 == 0 {
            Self::new(self.m.sqrt(), self.e / 2)
        } else {
            Self::new((2.0 * self.m).sqrt(), (self.e - 1).div_euclid(2))
        }
    }

    pub fn log2(self) -> f64 {
        self.m.abs().log2() + self.e as f64
    }

    /// `2^x` for arbitrarily large `|x|`.
    pub fn exp2(x: f64) -> Self {
        let e = x.floor();
        Self::new((x - e).exp2(), e as i32)
    }

    pub fn powi(self, mut k: i64) -> Self {
        let mut base = if k < 0 { Self::ONE / self } else { self };
        k = k.abs();
        let mut acc = Self::ONE;
        while k > 0 {
            if k & 1 == 1 {
                acc = acc * base;
            }
            base = base * base;
            k >>= 1;
        }
        acc
    }

    #[inline]
    pub fn max(self, o: Self) -> Self {
        if o > self { o } else { self }
    }

    #[inline]
    pub fn min(self, o: Self) -> Self {
        if o < self { o } else { self }
    }

    /// Parse decimal / scientific notation of any exponent, e.g. `1.5E-12345`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (mant, exp10) = match s.find(['e', 'E']) {
            Some(i) => (&s[..i], s[i + 1..].trim_start_matches('+').parse::<i64>().ok()?),
            None => (s, 0),
        };
        let m: f64 = mant.parse().ok()?;
        if !m.is_finite() {
            return None;
        }
        Some(Self::from(m) * Self::from(10.0).powi(exp10))
    }
}

impl From<f64> for FloatExp {
    #[inline]
    fn from(x: f64) -> Self {
        Self::new(x, 0)
    }
}

impl Add for FloatExp {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        let d = self.e as i64 - o.e as i64;
        if d > 64 {
            self
        } else if d < -64 {
            o
        } else if d >= 0 {
            Self::new(self.m + o.m * pow2(-d as i32), self.e)
        } else {
            Self::new(self.m * pow2(d as i32) + o.m, o.e)
        }
    }
}

impl Sub for FloatExp {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        self + (-o)
    }
}

impl Neg for FloatExp {
    type Output = Self;
    #[inline(always)]
    fn neg(self) -> Self {
        FloatExp { m: -self.m, e: self.e }
    }
}

impl Mul for FloatExp {
    type Output = Self;
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Self::new(self.m * o.m, self.e + o.e)
    }
}

impl Div for FloatExp {
    type Output = Self;
    #[inline]
    fn div(self, o: Self) -> Self {
        Self::new(self.m / o.m, self.e - o.e)
    }
}

impl PartialOrd for FloatExp {
    #[inline]
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        if self.m == 0.0 || o.m == 0.0 || (self.m > 0.0) != (o.m > 0.0) {
            return self.m.partial_cmp(&o.m);
        }
        let ord = self.e.cmp(&o.e).then(self.m.abs().partial_cmp(&o.m.abs())?);
        Some(if self.m > 0.0 { ord } else { ord.reverse() })
    }

    // Fast paths for the common case of comparing non-negative magnitudes.
    #[inline(always)]
    fn lt(&self, o: &Self) -> bool {
        if self.m >= 0.0 && o.m > 0.0 {
            self.e < o.e || (self.e == o.e && self.m < o.m)
        } else {
            matches!(self.partial_cmp(o), Some(Ordering::Less))
        }
    }

    #[inline(always)]
    fn gt(&self, o: &Self) -> bool {
        o.lt(self)
    }
}

impl fmt::Display for FloatExp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.m == 0.0 {
            return write!(f, "0");
        }
        let l10 = self.abs().log2() * std::f64::consts::LOG10_2;
        let mut e10 = l10.floor();
        let mut mant = 10f64.powf(l10 - e10);
        let prec = f.precision().unwrap_or(6);
        // Rounding can push the mantissa to 10.0.
        if format!("{mant:.prec$}").starts_with("10") {
            mant /= 10.0;
            e10 += 1.0;
        }
        let sign = if self.m < 0.0 { "-" } else { "" };
        write!(f, "{sign}{mant:.prec$}E{}", e10 as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_ops() {
        for &a in &[1.0, -2.5, 3e-300, 7e250, 0.1, -1e-310] {
            assert_eq!(FloatExp::from(a).to_f64(), a);
            for &b in &[1.0, -0.3, 2e-200, 5e100] {
                let (fa, fb) = (FloatExp::from(a), FloatExp::from(b));
                let rel = |x: f64, y: f64| x == y || (x - y).abs() <= 1e-15 * y.abs();
                assert!(rel((fa * fb).to_f64(), a * b));
                assert!(rel((fa + fb).to_f64(), a + b));
                assert!(rel((fa - fb).to_f64(), a - b));
                assert!(rel((fa / fb).to_f64(), a / b));
                assert_eq!(fa < fb, a < b);
            }
        }
        assert_eq!((FloatExp::ZERO + FloatExp::ONE).to_f64(), 1.0);
        assert!(FloatExp::ZERO < FloatExp::ONE && -FloatExp::ONE < FloatExp::ZERO);
        assert_eq!(FloatExp::from(16.0).sqrt().to_f64(), 4.0);
        assert_eq!(FloatExp::from(8.0).sqrt().to_f64(), 8f64.sqrt());
    }

    #[test]
    fn huge_exponents() {
        let z = FloatExp::parse("2.5E-5000").unwrap();
        assert!((z.log2() * std::f64::consts::LOG10_2 - (2.5f64.log10() - 5000.0)).abs() < 1e-9);
        assert_eq!(format!("{z:.4}"), "2.5000E-5000");
        assert_eq!(format!("{:.3}", FloatExp::from(9.9996)), "1.000E1");
        assert_eq!(z.to_f64(), 0.0);
        assert!(((z * FloatExp::parse("4E5000").unwrap()).to_f64() - 10.0).abs() < 1e-9);
    }
}
