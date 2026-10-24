//! fracst core: a fast deep-zoom Mandelbrot set engine.
//!
//! The pipeline per frame is: high-precision reference orbit
//! ([`reference`]) → bilinear approximation table ([`bla`]) → parallel
//! perturbation with rebasing for every pixel ([`perturb`], [`render`]) →
//! colouring ([`colour`]).

// `!(a < b)` is used on purpose: it treats NaN as failing the comparison.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod bla;
pub mod colour;
pub mod fixed;
pub mod floatexp;
pub mod image;
pub mod kfr;
pub mod newton;
pub mod num;
pub mod perturb;
pub mod reference;
pub mod render;
pub mod view;

pub use colour::{Colouring, Palette, Slopes, Transfer};
pub use fixed::Fixed;
pub use floatexp::FloatExp;
pub use render::{Frame, INTERIOR, Renderer, Settings, Stats, UNDECIDED, auto_floor};
pub use view::View;

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

/// Shared progress reporting and cancellation for a render in flight.
#[derive(Debug, Default)]
pub struct Progress {
    cancel: AtomicBool,
    stage: AtomicU8,
    pub done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub const IDLE: u8 = 0;
    pub const REFERENCE: u8 = 1;
    pub const BLA: u8 = 2;
    pub const PIXELS: u8 = 3;

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn set_stage(&self, stage: u8, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.stage.store(stage, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    /// Current stage and completed fraction of it.
    pub fn snapshot(&self) -> (u8, f64) {
        let total = self.total.load(Ordering::Relaxed).max(1);
        let done = self.done.load(Ordering::Relaxed).min(total);
        (self.stage.load(Ordering::Relaxed), done as f64 / total as f64)
    }
}
