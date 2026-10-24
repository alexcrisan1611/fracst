//! Bilinear approximation (BLA) tables for skipping perturbation iterations.
//!
//! While `|dz|` is small compared with the reference orbit, `l` iterations
//! starting at reference index `m` collapse into `dz <- A dz + B dc`
//! (Zhuoran's BLA). Single steps (`A = 2 Z_m`, `B = 1`) are merged pairwise
//! into a binary tree, so a pixel can skip up to `2^k` iterations at once.

use crate::num::{Complex, Cplx, Real};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Bla<C: Cplx> {
    pub a: C,
    pub b: C,
    /// Squared validity radius: the step applies while `|dz|^2 < r2`.
    pub r2: C::R,
    /// Number of iterations skipped.
    pub l: u32,
}

pub struct BlaTable<C: Cplx> {
    /// `levels[k][j]` skips up to `2^(k+1)` iterations from `m = 1 + j * 2^(k+1)`.
    /// Single steps are not stored: they cost the same as a perturbation step.
    levels: Vec<Vec<Bla<C>>>,
    /// Largest `r2` of the first level; no step is valid for larger `|dz|^2`.
    max_r2: C::R,
}

impl<T: Real> BlaTable<Complex<T>> {
    /// Build a table for a reference orbit with last index `len` (orbit values
    /// `z(0) ..= z(len)`), for pixels with `|dc| <= cmax`. `epsilon` is the
    /// tolerated relative error of a single linearised step.
    pub fn build(len: usize, z: impl Fn(usize) -> Complex<T> + Sync, cmax: T, epsilon: f64) -> Self {
        let eps = T::from_f64(epsilon);
        // During construction `r2` holds the (unsquared) radius.
        let single = |m: usize| {
            let zm = z(m);
            Bla { a: zm.mul2(), b: Complex::ONE, r2: eps * zm.abs(), l: 1 }
        };
        let merge = |x: &Bla<Complex<T>>, y: &Bla<Complex<T>>| {
            let a = y.a * x.a;
            let b = y.a * x.b + y.b;
            let xa = x.a.abs();
            let mut r = if xa > T::ZERO { x.r2.min(((y.r2 - x.b.abs() * cmax) / xa).max(T::ZERO)) } else { T::ZERO };
            if !(a.is_finite() && b.is_finite() && r.is_finite()) {
                r = T::ZERO;
            }
            Bla { a, b, r2: r, l: x.l + y.l }
        };

        // Single steps exist for m in 1..len.
        let singles = len.saturating_sub(1);
        let mut levels: Vec<Vec<Bla<Complex<T>>>> = Vec::new();
        if singles >= 2 {
            let level0 = (0..singles.div_ceil(2))
                .into_par_iter()
                .map(|j| {
                    let m = 1 + 2 * j;
                    let x = single(m);
                    if m + 1 < len { merge(&x, &single(m + 1)) } else { x }
                })
                .collect();
            levels.push(level0);
            while levels.last().unwrap().len() > 1 {
                let prev = levels.last().unwrap();
                let next = (0..prev.len().div_ceil(2))
                    .into_par_iter()
                    .map(|j| match prev.get(2 * j + 1) {
                        Some(y) => merge(&prev[2 * j], y),
                        None => prev[2 * j],
                    })
                    .collect();
                levels.push(next);
            }
        }
        for level in &mut levels {
            level.par_iter_mut().for_each(|b| b.r2 = b.r2 * b.r2);
        }
        Self::from_levels(levels)
    }
}

impl<C: Cplx> BlaTable<C> {
    fn from_levels(levels: Vec<Vec<Bla<C>>>) -> Self {
        let max_r2 = levels.first().map_or(C::R::ZERO, |l| l.iter().fold(C::R::ZERO, |m, b| m.max(b.r2)));
        BlaTable { levels, max_r2 }
    }

    /// An empty table: every lookup fails (plain perturbation).
    pub fn empty() -> Self {
        BlaTable { levels: Vec::new(), max_r2: C::R::ZERO }
    }

    /// The longest valid step at reference index `m` for `|dz|^2 = dz_norm`
    /// that skips at most `max_steps` iterations.
    #[inline(always)]
    pub fn lookup(&self, m: usize, dz_norm: C::R, max_steps: u64) -> Option<&Bla<C>> {
        if m == 0 || !(dz_norm < self.max_r2) {
            return None;
        }
        let i = m - 1;
        let usable = (i.trailing_zeros() as usize).min(self.levels.len());
        let mut best = None;
        // Radii shrink as levels grow, so stop at the first failure.
        for k in 0..usable {
            let Some(b) = self.levels[k].get(i >> (k + 1)) else { break };
            if !(dz_norm < b.r2) || b.l as u64 > max_steps {
                break;
            }
            best = Some(b);
        }
        best
    }

    /// Convert to another complex type, invalidating entries that do not fit.
    pub fn cast<D: Cplx>(&self) -> BlaTable<D> {
        let levels = self
            .levels
            .iter()
            .map(|level| {
                level
                    .par_iter()
                    .map(|b| {
                        let (a, bb) = (D::from_cfe(b.a.to_cfe()), D::from_cfe(b.b.to_cfe()));
                        let mut r2 = D::R::from_fe(b.r2.to_fe());
                        if !(a.is_finite() && bb.is_finite()) {
                            r2 = D::R::ZERO;
                        } else if !r2.is_finite() {
                            r2 = D::R::from_f64(f64::MAX);
                        }
                        Bla { a, b: bb, r2, l: b.l }
                    })
                    .collect()
            })
            .collect();
        BlaTable::from_levels(levels)
    }

    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    pub fn memory_bytes(&self) -> usize {
        self.levels.iter().map(|l| l.capacity() * size_of::<Bla<C>>()).sum()
    }
}
