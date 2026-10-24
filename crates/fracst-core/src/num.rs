//! Scalar abstraction over `f64` and [`FloatExp`], and a small complex type.

use crate::floatexp::FloatExp;
use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Neg, Sub};

pub trait Real:
    Copy
    + Send
    + Sync
    + Debug
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + 'static
{
    const ZERO: Self;
    const ONE: Self;
    fn from_f64(x: f64) -> Self;
    fn from_fe(x: FloatExp) -> Self;
    fn to_f64(self) -> f64;
    fn to_fe(self) -> FloatExp;
    fn mul2(self) -> Self;
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    fn is_finite(self) -> bool;
    #[inline]
    fn max(self, o: Self) -> Self {
        if o > self { o } else { self }
    }
    #[inline]
    fn min(self, o: Self) -> Self {
        if o < self { o } else { self }
    }
}

impl Real for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    #[inline(always)]
    fn from_f64(x: f64) -> Self {
        x
    }
    #[inline(always)]
    fn from_fe(x: FloatExp) -> Self {
        x.to_f64()
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self
    }
    #[inline(always)]
    fn to_fe(self) -> FloatExp {
        FloatExp::from(self)
    }
    #[inline(always)]
    fn mul2(self) -> Self {
        self * 2.0
    }
    #[inline(always)]
    fn abs(self) -> Self {
        f64::abs(self)
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        f64::sqrt(self)
    }
    #[inline(always)]
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

impl Real for FloatExp {
    const ZERO: Self = FloatExp::ZERO;
    const ONE: Self = FloatExp::ONE;
    #[inline(always)]
    fn from_f64(x: f64) -> Self {
        FloatExp::from(x)
    }
    #[inline(always)]
    fn from_fe(x: FloatExp) -> Self {
        x
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        FloatExp::to_f64(self)
    }
    #[inline(always)]
    fn to_fe(self) -> FloatExp {
        self
    }
    #[inline(always)]
    fn mul2(self) -> Self {
        FloatExp::mul2(self)
    }
    #[inline(always)]
    fn abs(self) -> Self {
        FloatExp::abs(self)
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        FloatExp::sqrt(self)
    }
    #[inline(always)]
    fn is_finite(self) -> bool {
        self.parts().0.is_finite()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Complex<T> {
    pub re: T,
    pub im: T,
}

impl<T: Real> Complex<T> {
    pub const ZERO: Self = Complex { re: T::ZERO, im: T::ZERO };
    pub const ONE: Self = Complex { re: T::ONE, im: T::ZERO };

    #[inline(always)]
    pub fn new(re: T, im: T) -> Self {
        Complex { re, im }
    }

    #[inline(always)]
    pub fn norm(self) -> T {
        self.re * self.re + self.im * self.im
    }

    #[inline(always)]
    pub fn abs(self) -> T {
        self.norm().sqrt()
    }

    #[inline(always)]
    pub fn mul2(self) -> Self {
        Complex { re: self.re.mul2(), im: self.im.mul2() }
    }

    #[inline(always)]
    pub fn scale(self, k: T) -> Self {
        Complex { re: self.re * k, im: self.im * k }
    }

    #[inline(always)]
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }

    #[inline(always)]
    pub fn to_f64(self) -> Complex<f64> {
        Complex { re: self.re.to_f64(), im: self.im.to_f64() }
    }

    #[inline(always)]
    pub fn to_fe(self) -> Complex<FloatExp> {
        Complex { re: self.re.to_fe(), im: self.im.to_fe() }
    }

    #[inline(always)]
    pub fn cast<U: Real>(self) -> Complex<U> {
        Complex { re: U::from_fe(self.re.to_fe()), im: U::from_fe(self.im.to_fe()) }
    }
}

impl<T: Real> Add for Complex<T> {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        Complex { re: self.re + o.re, im: self.im + o.im }
    }
}

impl<T: Real> Sub for Complex<T> {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        Complex { re: self.re - o.re, im: self.im - o.im }
    }
}

impl<T: Real> Mul for Complex<T> {
    type Output = Self;
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Complex { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

impl<T: Real> Div for Complex<T> {
    type Output = Self;
    #[inline]
    fn div(self, o: Self) -> Self {
        let d = o.norm();
        Complex { re: (self.re * o.re + self.im * o.im) / d, im: (self.im * o.re - self.re * o.im) / d }
    }
}

impl<T: Real> Neg for Complex<T> {
    type Output = Self;
    #[inline(always)]
    fn neg(self) -> Self {
        Complex { re: -self.re, im: -self.im }
    }
}

/// What the per-pixel loop and BLA tables need from a complex type: plain
/// [`Complex<f64>`], or [`CExp`] for extended range.
pub trait Cplx:
    Copy + Send + Sync + Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + 'static
{
    /// Type of norms, radii and other real quantities.
    type R: Real;
    const ZERO: Self;
    const ONE: Self;
    fn norm(self) -> Self::R;
    fn mul2(self) -> Self;
    fn scale(self, k: Self::R) -> Self;
    fn from_real(x: Self::R) -> Self;
    fn to_c64(self) -> Complex<f64>;
    fn to_cfe(self) -> Complex<FloatExp>;
    fn from_cfe(c: Complex<FloatExp>) -> Self;
    fn is_finite(self) -> bool;
}

impl<T: Real> Cplx for Complex<T> {
    type R = T;
    const ZERO: Self = Complex { re: T::ZERO, im: T::ZERO };
    const ONE: Self = Complex { re: T::ONE, im: T::ZERO };
    #[inline(always)]
    fn norm(self) -> T {
        Complex::norm(self)
    }
    #[inline(always)]
    fn mul2(self) -> Self {
        Complex::mul2(self)
    }
    #[inline(always)]
    fn scale(self, k: T) -> Self {
        Complex::scale(self, k)
    }
    #[inline(always)]
    fn from_real(x: T) -> Self {
        Complex { re: x, im: T::ZERO }
    }
    #[inline(always)]
    fn to_c64(self) -> Complex<f64> {
        self.to_f64()
    }
    #[inline(always)]
    fn to_cfe(self) -> Complex<FloatExp> {
        self.to_fe()
    }
    #[inline(always)]
    fn from_cfe(c: Complex<FloatExp>) -> Self {
        c.cast()
    }
    #[inline(always)]
    fn is_finite(self) -> bool {
        Complex::is_finite(self)
    }
}

/// Extended-range complex number with one shared exponent: `(re + i im) * 2^e`.
///
/// Much cheaper than a pair of [`FloatExp`]s: arithmetic is plain `f64` and
/// renormalisation is lazy, happening only when the larger mantissa leaves
/// `2^±480`. Products of in-range mantissas therefore never overflow, and
/// anything that underflows is negligible (render threads flush subnormals).
#[derive(Clone, Copy, Debug)]
pub struct CExp {
    re: f64,
    im: f64,
    e: i32,
}

const CEXP_LO: u64 = 1023 - 480;
const CEXP_HI: u64 = 1023 + 480;
const CEXP_ZERO_E: i32 = i32::MIN / 4;

#[inline(always)]
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

impl CExp {
    #[inline(always)]
    pub fn new(re: f64, im: f64, e: i32) -> Self {
        let biased = (re.abs().max(im.abs()).to_bits() >> 52) & 0x7ff;
        if (CEXP_LO..=CEXP_HI).contains(&biased) { CExp { re, im, e } } else { Self::renormalise(re, im, e) }
    }

    #[cold]
    #[inline(never)]
    fn renormalise(re: f64, im: f64, e: i32) -> Self {
        let m = re.abs().max(im.abs());
        if m == 0.0 || !m.is_finite() {
            return if m == 0.0 { <Self as Cplx>::ZERO } else { CExp { re, im, e } };
        }
        let (re, im, e) = if m < f64::MIN_POSITIVE { (re * pow2(64), im * pow2(64), e - 64) } else { (re, im, e) };
        let k = (((re.abs().max(im.abs())).to_bits() >> 52) & 0x7ff) as i32 - 1023;
        let s = pow2(-k);
        CExp { re: re * s, im: im * s, e: e + k }
    }
}

impl Add for CExp {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        // Mantissas lie within 2^±480, so beyond 1021 bits apart the smaller
        // operand is below 2^-60 of the larger.
        let d = self.e - o.e;
        if d >= 0 {
            if d > 1021 {
                return self;
            }
            let s = pow2(-d);
            CExp::new(self.re + o.re * s, self.im + o.im * s, self.e)
        } else {
            if d < -1021 {
                return o;
            }
            let s = pow2(d);
            CExp::new(self.re * s + o.re, self.im * s + o.im, o.e)
        }
    }
}

impl Sub for CExp {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        self + CExp { re: -o.re, im: -o.im, e: o.e }
    }
}

impl Mul for CExp {
    type Output = Self;
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        CExp::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re, self.e + o.e)
    }
}

impl Cplx for CExp {
    type R = FloatExp;
    const ZERO: Self = CExp { re: 0.0, im: 0.0, e: CEXP_ZERO_E };
    const ONE: Self = CExp { re: 1.0, im: 0.0, e: 0 };
    #[inline(always)]
    fn norm(self) -> FloatExp {
        FloatExp::new(self.re * self.re + self.im * self.im, 2 * self.e)
    }
    #[inline(always)]
    fn mul2(self) -> Self {
        CExp { e: self.e + 1, ..self }
    }
    #[inline(always)]
    fn scale(self, k: FloatExp) -> Self {
        let (m, e) = k.parts();
        CExp::new(self.re * m, self.im * m, self.e + e)
    }
    #[inline(always)]
    fn from_real(x: FloatExp) -> Self {
        let (m, e) = x.parts();
        if m == 0.0 { Self::ZERO } else { CExp { re: m, im: 0.0, e } }
    }
    #[inline]
    fn to_c64(self) -> Complex<f64> {
        Complex { re: FloatExp::new(self.re, self.e).to_f64(), im: FloatExp::new(self.im, self.e).to_f64() }
    }
    #[inline]
    fn to_cfe(self) -> Complex<FloatExp> {
        Complex { re: FloatExp::new(self.re, self.e), im: FloatExp::new(self.im, self.e) }
    }
    #[inline]
    fn from_cfe(c: Complex<FloatExp>) -> Self {
        let ((mr, er), (mi, ei)) = (c.re.parts(), c.im.parts());
        let e = match (mr == 0.0, mi == 0.0) {
            (true, true) => return Self::ZERO,
            (false, true) => er,
            (true, false) => ei,
            (false, false) => er.max(ei),
        };
        let part = |m: f64, k: i32| if m == 0.0 || k - e < -1000 { 0.0 } else { m * pow2(k - e) };
        CExp::new(part(mr, er), part(mi, ei), e)
    }
    #[inline(always)]
    fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Complex<FloatExp>, b: Complex<FloatExp>) -> bool {
        let d = (a - b).abs();
        let m = a.abs().max(b.abs());
        d <= m * FloatExp::from(1e-14) || m.is_zero()
    }

    #[test]
    fn cexp_matches_floatexp() {
        let vals = [
            Complex::new(FloatExp::from(1.5), FloatExp::from(-0.25)),
            Complex::new(FloatExp::parse("3e-500").unwrap(), FloatExp::parse("-1e-501").unwrap()),
            Complex::new(FloatExp::parse("7e400").unwrap(), FloatExp::ZERO),
            Complex::new(FloatExp::ZERO, FloatExp::parse("-2e-1300").unwrap()),
            Complex::new(FloatExp::from(1e-300), FloatExp::from(1.0)),
            Complex::ZERO,
        ];
        for &a in &vals {
            assert!(close(CExp::from_cfe(a).to_cfe(), a));
            for &b in &vals {
                let (ca, cb) = (CExp::from_cfe(a), CExp::from_cfe(b));
                assert!(close((ca * cb).to_cfe(), a * b), "{a:?} * {b:?}");
                assert!(close((ca + cb).to_cfe(), a + b), "{a:?} + {b:?}");
                assert!(close((ca - cb).to_cfe(), a - b), "{a:?} - {b:?}");
                let (n1, n2) = (ca.norm(), a.norm());
                assert!((n1 - n2).abs() <= n2 * FloatExp::from(1e-14));
            }
        }
        // Repeated squaring far beyond f64 range and back.
        let mut x = CExp::from_cfe(Complex::new(FloatExp::from(0.5), FloatExp::from(0.5)));
        for _ in 0..12 {
            x = x * x;
        }
        let expect = Complex::new(FloatExp::from(0.5), FloatExp::from(0.5)).to_fe();
        let mut y = expect;
        for _ in 0..12 {
            y = y * y;
        }
        assert!(close(x.to_cfe(), y));
    }
}
