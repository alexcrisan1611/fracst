//! Arbitrary-precision signed fixed-point numbers for reference orbits.
//!
//! A [`Fixed`] is a two's complement integer of `n` little-endian 64-bit limbs,
//! read with `n - 1` fractional limbs: the top limb is the signed integer part.
//! Mandelbrot reference orbits never leave `|z| < 2^62`, so fixed point is both
//! simpler and faster than a floating-point bignum. All operands in an
//! arithmetic operation must have the same limb count.

use crate::floatexp::FloatExp;
use std::fmt;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Fixed {
    limbs: Vec<u64>,
}

/// Number of limbs needed to resolve features of size `2^-bits` with headroom.
pub fn limbs_for_bits(bits: u64) -> usize {
    // One integer limb plus at least 64 guard bits beyond the requested depth.
    1 + bits.div_ceil(64) as usize + 1
}

impl Fixed {
    pub fn zero(n: usize) -> Self {
        assert!(n >= 2, "Fixed needs at least one integer and one fractional limb");
        Fixed { limbs: vec![0; n] }
    }

    pub fn limbs(&self) -> usize {
        self.limbs.len()
    }

    /// The limbs, least significant first (the last one is the signed integer part).
    pub fn as_limbs(&self) -> &[u64] {
        &self.limbs
    }

    /// The number with these limbs (see [`Fixed::as_limbs`]); `None` with fewer than two.
    pub fn from_limbs(limbs: Vec<u64>) -> Option<Self> {
        (limbs.len() >= 2).then_some(Fixed { limbs })
    }

    /// Fractional precision in bits.
    pub fn precision_bits(&self) -> u64 {
        64 * (self.limbs.len() as u64 - 1)
    }

    pub fn is_negative(&self) -> bool {
        is_neg(&self.limbs)
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Change the precision, keeping the value (truncating towards -inf when shrinking).
    pub fn with_limbs(&self, n: usize) -> Self {
        assert!(n >= 2);
        let old = self.limbs.len();
        let limbs = if n >= old {
            let mut v = vec![0; n - old];
            v.extend_from_slice(&self.limbs);
            v
        } else {
            self.limbs[old - n..].to_vec()
        };
        Fixed { limbs }
    }

    pub fn from_f64(x: f64, n: usize) -> Self {
        Self::from_floatexp(FloatExp::from(x), n)
    }

    pub fn from_floatexp(x: FloatExp, n: usize) -> Self {
        let mut out = Fixed::zero(n);
        let (m, e) = x.parts();
        if m == 0.0 || !m.is_finite() {
            return out;
        }
        // m is in [1, 2): m = mant * 2^-52 with a 53-bit integer mantissa.
        let mant = (m.abs() * (1u64 << 52) as f64) as u64;
        let shift = e as i64 - 52 + 64 * (n as i64 - 1);
        place_bits(&mut out.limbs, mant, shift);
        if m < 0.0 {
            negate(&mut out.limbs);
        }
        out
    }

    pub fn to_floatexp(&self) -> FloatExp {
        let mut scratch = vec![0; self.limbs.len()];
        limbs_to_floatexp(&self.limbs, &mut scratch)
    }

    pub fn to_f64(&self) -> f64 {
        self.to_floatexp().to_f64()
    }

    /// Parse a decimal string such as `-1.25`, `0.5e-3` or `3E+2`.
    pub fn parse(s: &str, n: usize) -> Option<Self> {
        let s = s.trim();
        let (neg, s) = match s.as_bytes().first()? {
            b'-' => (true, &s[1..]),
            b'+' => (false, &s[1..]),
            _ => (false, s),
        };
        let (mant, exp10) = match s.find(['e', 'E']) {
            Some(i) => (&s[..i], s[i + 1..].parse::<i64>().ok()?),
            None => (s, 0),
        };
        let (int_str, frac_str) = match mant.find('.') {
            Some(i) => (&mant[..i], &mant[i + 1..]),
            None => (mant, ""),
        };
        if int_str.is_empty() && frac_str.is_empty() {
            return None;
        }
        let mut digits = Vec::with_capacity(int_str.len() + frac_str.len());
        for c in int_str.bytes().chain(frac_str.bytes()) {
            if !c.is_ascii_digit() {
                return None;
            }
            digits.push(c - b'0');
        }
        // value = 0.d0 d1 d2 ... * 10^point
        let point = int_str.len() as i64 + exp10;
        let mut out = Fixed::zero(n);
        // Integer part: digits[..point].
        let mut int_part: u64 = 0;
        for i in 0..point.max(0) {
            let d = digits.get(i as usize).copied().unwrap_or(0) as u64;
            int_part = int_part.checked_mul(10)?.checked_add(d)?;
        }
        if int_part > (1 << 62) {
            return None;
        }
        // Fractional part: process digits from least significant: x = (d + x) / 10.
        let frac_start = point.max(0) as usize;
        let leading_zeros = (-point).max(0) as usize;
        let max_digits = (out.precision_bits() as f64 * std::f64::consts::LOG10_2) as usize + 2;
        let frac_digits = digits.get(frac_start..).unwrap_or(&[]);
        let top = n - 1;
        if leading_zeros < max_digits {
            let usable = frac_digits.len().min(max_digits - leading_zeros);
            for &d in frac_digits[..usable].iter().rev() {
                out.limbs[top] = d as u64;
                div_small(&mut out.limbs, 10);
            }
            for _ in 0..leading_zeros {
                div_small(&mut out.limbs, 10);
            }
        }
        out.limbs[top] = int_part;
        if neg {
            negate(&mut out.limbs);
        }
        Some(out)
    }

    /// Decimal representation with `frac_digits` digits after the point
    /// (trailing zeros trimmed).
    pub fn to_decimal(&self, frac_digits: usize) -> String {
        let neg = self.is_negative();
        let mut mag = self.limbs.clone();
        if neg {
            negate(&mut mag);
        }
        let top = mag.len() - 1;
        // Round to nearest: add half a unit in the last printed digit.
        let mut half = vec![0u64; mag.len()];
        half[top] = 5;
        for _ in 0..=frac_digits.min(self.precision_bits() as usize) {
            div_small(&mut half, 10);
        }
        add_assign(&mut mag, &half);
        let mut s = String::new();
        if neg {
            s.push('-');
        }
        s.push_str(&mag[top].to_string());
        mag[top] = 0;
        if mag.iter().any(|&l| l != 0) && frac_digits > 0 {
            s.push('.');
            for _ in 0..frac_digits {
                mag[top] = 0;
                mul_small(&mut mag, 10);
                s.push((b'0' + mag[top] as u8) as char);
            }
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        if s == "-0" { "0".into() } else { s }
    }

    pub fn add(&self, o: &Fixed) -> Fixed {
        let mut r = self.clone();
        add_assign(&mut r.limbs, &o.limbs);
        r
    }

    pub fn sub(&self, o: &Fixed) -> Fixed {
        let mut r = self.clone();
        sub_assign(&mut r.limbs, &o.limbs);
        r
    }

    pub fn neg(&self) -> Fixed {
        let mut r = self.clone();
        negate(&mut r.limbs);
        r
    }

    pub fn mul(&self, o: &Fixed) -> Fixed {
        let n = self.limbs.len();
        assert_eq!(n, o.limbs.len());
        let mut a = self.limbs.clone();
        let mut b = o.limbs.clone();
        let neg = is_neg(&a) != is_neg(&b);
        if is_neg(&a) {
            negate(&mut a);
        }
        if is_neg(&b) {
            negate(&mut b);
        }
        let mut out = Fixed::zero(n);
        let mut prod = vec![0; 2 * n];
        mul_mag(&a, &b, &mut out.limbs, &mut prod);
        if neg {
            negate(&mut out.limbs);
        }
        out
    }

    pub fn sqr(&self) -> Fixed {
        let n = self.limbs.len();
        let mut a = self.limbs.clone();
        if is_neg(&a) {
            negate(&mut a);
        }
        let mut out = Fixed::zero(n);
        let mut prod = vec![0; 2 * n];
        sqr_mag(&a, &mut out.limbs, &mut prod);
        out
    }

    pub(crate) fn raw(&self) -> &[u64] {
        &self.limbs
    }

    #[cfg(test)]
    pub(crate) fn raw_mut(&mut self) -> &mut [u64] {
        &mut self.limbs
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = (self.precision_bits() as f64 * std::f64::consts::LOG10_2) as usize;
        write!(f, "Fixed({})", self.to_decimal(digits.min(40)))
    }
}

/// Convert two's complement fixed-point limbs to [`FloatExp`], using `scratch`
/// (same length) for the magnitude of negative values.
pub(crate) fn limbs_to_floatexp(v: &[u64], scratch: &mut [u64]) -> FloatExp {
    let neg = is_neg(v);
    let mag: &[u64] = if neg {
        scratch.copy_from_slice(v);
        negate(scratch);
        scratch
    } else {
        v
    };
    let n = mag.len();
    let Some(t) = mag.iter().rposition(|&l| l != 0) else {
        return FloatExp::ZERO;
    };
    let lz = mag[t].leading_zeros();
    let mut window = mag[t] << lz;
    if lz > 0 && t > 0 {
        window |= mag[t - 1] >> (64 - lz);
    }
    let exp = 64 * t as i64 - lz as i64 - 64 * (n as i64 - 1);
    let f = FloatExp::new(window as f64, exp as i32);
    if neg { -f } else { f }
}

/// Place the 64-bit value `v * 2^shift` (shift relative to limb 0, bit 0) into zeroed limbs.
fn place_bits(limbs: &mut [u64], v: u64, shift: i64) {
    let n = limbs.len() as i64;
    if shift <= -64 {
        return;
    }
    if shift < 0 {
        limbs[0] |= v >> (-shift);
        return;
    }
    let idx = shift / 64;
    let bit = (shift % 64) as u32;
    if idx < n {
        limbs[idx as usize] |= v << bit;
    }
    if bit != 0 && idx + 1 < n {
        limbs[idx as usize + 1] |= v >> (64 - bit);
    }
}

#[inline]
pub(crate) fn is_neg(a: &[u64]) -> bool {
    (a[a.len() - 1] as i64) < 0
}

#[inline]
pub(crate) fn negate(a: &mut [u64]) {
    let mut carry = 1u64;
    for l in a.iter_mut() {
        let (v, c) = (!*l).overflowing_add(carry);
        *l = v;
        carry = c as u64;
    }
}

#[inline]
pub(crate) fn add_assign(a: &mut [u64], b: &[u64]) {
    let mut carry = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (s1, c1) = x.overflowing_add(y);
        let (s2, c2) = s1.overflowing_add(carry as u64);
        *x = s2;
        carry = c1 | c2;
    }
}

#[inline]
pub(crate) fn sub_assign(a: &mut [u64], b: &[u64]) {
    let mut borrow = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (s1, b1) = x.overflowing_sub(y);
        let (s2, b2) = s1.overflowing_sub(borrow as u64);
        *x = s2;
        borrow = b1 | b2;
    }
}

/// `out = a + b`
#[inline]
pub(crate) fn add_into(out: &mut [u64], a: &[u64], b: &[u64]) {
    out.copy_from_slice(a);
    add_assign(out, b);
}

/// Copy `|a|` into `out`.
#[inline]
pub(crate) fn abs_into(out: &mut [u64], a: &[u64]) {
    out.copy_from_slice(a);
    if is_neg(out) {
        negate(out);
    }
}

fn div_small(a: &mut [u64], d: u64) {
    let mut rem: u128 = 0;
    for l in a.iter_mut().rev() {
        let cur = (rem << 64) | *l as u128;
        *l = (cur / d as u128) as u64;
        rem = cur % d as u128;
    }
}

fn mul_small(a: &mut [u64], m: u64) {
    let mut carry: u128 = 0;
    for l in a.iter_mut() {
        let t = *l as u128 * m as u128 + carry;
        *l = t as u64;
        carry = t >> 64;
    }
}

/// `out = (a * b) >> (64 * (n - 1))` for non-negative `a`, `b`.
///
/// Partial products below limb `n - 2` are skipped (one guard limb), which
/// costs at most a few units in the last place but halves the work.
pub(crate) fn mul_mag(a: &[u64], b: &[u64], out: &mut [u64], prod: &mut [u64]) {
    let n = a.len();
    debug_assert!(b.len() == n && out.len() == n && prod.len() >= 2 * n);
    let prod = &mut prod[..2 * n];
    prod.fill(0);
    let lo = n.saturating_sub(2);
    for i in 0..n {
        let ai = a[i] as u128;
        if ai == 0 {
            continue;
        }
        let mut carry: u128 = 0;
        for j in lo.saturating_sub(i)..n {
            let t = prod[i + j] as u128 + ai * b[j] as u128 + carry;
            prod[i + j] = t as u64;
            carry = t >> 64;
        }
        prod[i + n] = carry as u64;
    }
    out.copy_from_slice(&prod[n - 1..2 * n - 1]);
}

/// `out = (a * a) >> (64 * (n - 1))` for non-negative `a`; about half the work of [`mul_mag`].
pub(crate) fn sqr_mag(a: &[u64], out: &mut [u64], prod: &mut [u64]) {
    let n = a.len();
    debug_assert!(out.len() == n && prod.len() >= 2 * n);
    let prod = &mut prod[..2 * n];
    prod.fill(0);
    let lo = n.saturating_sub(2);
    // Off-diagonal products a[i] * a[j] for i < j.
    for i in 0..n {
        let ai = a[i] as u128;
        let j0 = (i + 1).max(lo.saturating_sub(i));
        if ai == 0 || j0 >= n {
            continue;
        }
        let mut carry: u128 = 0;
        for j in j0..n {
            let t = prod[i + j] as u128 + ai * a[j] as u128 + carry;
            prod[i + j] = t as u64;
            carry = t >> 64;
        }
        prod[i + n] = carry as u64;
    }
    // Double them.
    let mut top = 0u64;
    for p in prod.iter_mut() {
        let v = *p;
        *p = (v << 1) | top;
        top = v >> 63;
    }
    // Add the squares on the diagonal.
    let mut carry: u128 = 0;
    for i in 0..n {
        if 2 * i + 1 < lo {
            continue;
        }
        let sq = a[i] as u128 * a[i] as u128;
        let t0 = prod[2 * i] as u128 + (sq as u64) as u128 + carry;
        prod[2 * i] = t0 as u64;
        let t1 = prod[2 * i + 1] as u128 + (sq >> 64) + (t0 >> 64);
        prod[2 * i + 1] = t1 as u64;
        carry = t1 >> 64;
    }
    out.copy_from_slice(&prod[n - 1..2 * n - 1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Fixed, b: &Fixed, ulps: u64) -> bool {
        let d = a.sub(b);
        let d = if d.is_negative() { d.neg() } else { d };
        d.raw()[1..].iter().all(|&l| l == 0) && d.raw()[0] <= ulps
    }

    #[test]
    fn parse_and_format_roundtrip() {
        let s = "-1.7400623825793399052208441670658256382966417204361718668798624184611829";
        let x = Fixed::parse(s, 6).unwrap();
        assert_eq!(x.to_decimal(70), s);
        assert_eq!(Fixed::parse("0.5", 3).unwrap().to_f64(), 0.5);
        assert_eq!(Fixed::parse("-2", 3).unwrap().to_f64(), -2.0);
        assert_eq!(Fixed::parse("1.5e-3", 3).unwrap().to_f64(), 1.5e-3);
        assert_eq!(Fixed::parse("25E-1", 3).unwrap().to_f64(), 2.5);
        assert_eq!(Fixed::parse(".25", 3).unwrap().to_f64(), 0.25);
        assert_eq!(Fixed::parse("0", 3).unwrap().to_decimal(10), "0");
        assert!(Fixed::parse("abc", 3).is_none());
        assert!(Fixed::parse("", 3).is_none());
    }

    #[test]
    fn tiny_values_keep_their_exponent() {
        let x = Fixed::parse("3e-1000", limbs_for_bits(3400)).unwrap();
        let fe = x.to_floatexp();
        let (m, e) = fe.parts();
        let log10 = (m.log2() + e as f64) * std::f64::consts::LOG10_2;
        assert!((log10 - (3f64.log10() - 1000.0)).abs() < 1e-9, "{log10}");
    }

    #[test]
    fn f64_conversion_is_exact() {
        for &v in &[0.0, 1.0, -1.0, 0.1, -0.75, 1.0 / 3.0, 1e-300, -3.5e-200, 2.0f64.powi(-1000)] {
            let x = Fixed::from_f64(v, 18);
            assert_eq!(x.to_f64(), v, "{v}");
        }
    }

    #[test]
    fn arithmetic_matches_f64() {
        let vals = [0.3, -0.7, 1.25, -1.9999, 1e-5, -3e-9];
        for &a in &vals {
            for &b in &vals {
                let fa = Fixed::from_f64(a, 4);
                let fb = Fixed::from_f64(b, 4);
                assert!((fa.add(&fb).to_f64() - (a + b)).abs() <= 1e-15 * (a + b).abs().max(1e-300));
                assert!((fa.sub(&fb).to_f64() - (a - b)).abs() <= 1e-15 * (a - b).abs().max(1e-300));
                let p = fa.mul(&fb).to_f64();
                assert!((p - a * b).abs() <= 1e-15 * (a * b).abs(), "{a} * {b} = {p}");
            }
        }
    }

    #[test]
    fn sqr_matches_mul() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for n in [2, 3, 5, 8, 17, 40] {
            for _ in 0..50 {
                let mut x = Fixed::zero(n);
                for l in x.raw_mut().iter_mut() {
                    *l = rnd();
                }
                x.raw_mut()[n - 1] = rnd() % 4; // keep |x| < 4
                if rnd() & 1 == 1 {
                    x = x.neg();
                }
                assert!(close(&x.sqr(), &x.mul(&x), 2 * n as u64), "n={n}");
            }
        }
    }

    #[test]
    fn high_precision_identity() {
        // (1/3)^2 * 9 == 1 to within a few ulps at 2000 bits.
        let n = 33;
        let third = Fixed::parse(&format!("0.{}", "3".repeat(700)), n).unwrap();
        let nine = Fixed::from_f64(9.0, n);
        let one = third.sqr().mul(&nine);
        assert!(close(&one, &Fixed::from_f64(1.0, n), 1 << 12), "{one:?}");
    }
}
