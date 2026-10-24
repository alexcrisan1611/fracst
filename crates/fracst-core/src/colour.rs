//! Colouring of raw iteration data: a 1024-entry palette interpolated from
//! key colours, indexed by the (transferred) smooth iteration count divided
//! by `iter_div`, plus neighbour-difference "slopes" shading. The parameters
//! match the colour keys of `.kfr` location files.

use crate::render::Frame;
use rayon::prelude::*;
use std::f64::consts::{FRAC_PI_2, PI};

pub const PALETTE_SIZE: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    keys: Vec<[u8; 3]>,
    table: Vec<[f32; 3]>,
}

impl Palette {
    pub fn new(keys: Vec<[u8; 3]>) -> Self {
        let keys = if keys.is_empty() { vec![[0, 0, 0], [255, 255, 255]] } else { keys };
        let n = keys.len();
        // Neighbouring keys are blended with a sinusoidal ease.
        let table = (0..PALETTE_SIZE)
            .map(|i| {
                let t = i as f64 * n as f64 / PALETTE_SIZE as f64;
                let p = t.floor();
                let u = ((t - p - 0.5) * PI).sin() / 2.0 + 0.5;
                let (a, b) = (keys[p as usize % n], keys[(p as usize + 1) % n]);
                std::array::from_fn(|c| ((a[c] as f64 * (1.0 - u) + b[c] as f64 * u) / 255.0) as f32)
            })
            .collect();
        Palette { keys, table }
    }

    /// The default palette.
    pub fn classic() -> Self {
        Self::new(vec![
            [255, 255, 255],
            [64, 0, 128],
            [0, 0, 160],
            [0, 128, 192],
            [0, 128, 64],
            [255, 255, 0],
            [255, 128, 64],
            [255, 0, 0],
        ])
    }

    /// Built-in palettes for quick selection.
    pub fn presets() -> Vec<(&'static str, Palette)> {
        vec![
            ("Classic", Self::classic()),
            (
                "Ember",
                Self::new(vec![[0, 0, 0], [96, 16, 0], [224, 96, 0], [255, 224, 128], [255, 255, 255], [128, 64, 16]]),
            ),
            (
                "Ocean",
                Self::new(vec![[0, 8, 32], [0, 64, 128], [64, 192, 224], [240, 255, 255], [32, 96, 160], [8, 24, 64]]),
            ),
            (
                "Rainbow",
                Self::new(vec![[255, 0, 0], [255, 160, 0], [255, 255, 0], [0, 200, 0], [0, 128, 255], [128, 0, 255]]),
            ),
            ("Monochrome", Self::new(vec![[0, 0, 0], [255, 255, 255]])),
            (
                "Pastel",
                Self::new(vec![[255, 214, 224], [255, 245, 200], [200, 255, 220], [200, 230, 255], [230, 210, 255]]),
            ),
        ]
    }

    /// A random palette of `n` key colours (deterministic for a given seed).
    pub fn random(seed: u64, n: usize) -> Self {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        };
        Self::new((0..n.max(2)).map(|_| [next(), next(), next()]).collect())
    }

    pub fn keys(&self) -> &[[u8; 3]] {
        &self.keys
    }

    #[inline]
    fn sample(&self, index: f64, smooth: bool) -> [f32; 3] {
        let i = index.floor();
        let a = self.table[(i as i64).rem_euclid(PALETTE_SIZE as i64) as usize];
        if !smooth {
            return a;
        }
        let b = self.table[(i as i64 + 1).rem_euclid(PALETTE_SIZE as i64) as usize];
        let f = (index - i) as f32;
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * f)
    }
}

/// Transfer functions applied to the iteration count (`ColorMethod` in `.kfr` files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Transfer {
    #[default]
    Linear,
    Sqrt,
    Cbrt,
    FourthRoot,
    Log,
    LogLog,
}

impl Transfer {
    pub const ALL: [Transfer; 6] =
        [Transfer::Linear, Transfer::Sqrt, Transfer::Cbrt, Transfer::FourthRoot, Transfer::Log, Transfer::LogLog];

    pub fn name(self) -> &'static str {
        match self {
            Transfer::Linear => "Linear",
            Transfer::Sqrt => "Square root",
            Transfer::Cbrt => "Cube root",
            Transfer::FourthRoot => "Fourth root",
            Transfer::Log => "Logarithm",
            Transfer::LogLog => "Log-log",
        }
    }

    /// The `ColorMethod` number used in `.kfr` files.
    pub fn method_code(self) -> u32 {
        match self {
            Transfer::Linear => 0,
            Transfer::Sqrt => 1,
            Transfer::Cbrt => 2,
            Transfer::Log => 3,
            Transfer::LogLog => 9,
            Transfer::FourthRoot => 11,
        }
    }

    pub fn from_method_code(m: u32) -> Self {
        match m {
            1 => Transfer::Sqrt,
            2 => Transfer::Cbrt,
            3 => Transfer::Log,
            9 => Transfer::LogLog,
            11 => Transfer::FourthRoot,
            _ => Transfer::Linear,
        }
    }

    #[inline]
    fn apply(self, x: f64) -> f64 {
        match self {
            Transfer::Linear => x,
            Transfer::Sqrt => x.max(0.0).sqrt(),
            Transfer::Cbrt => x.cbrt(),
            Transfer::FourthRoot => x.max(0.0).sqrt().sqrt(),
            Transfer::Log => x.max(1.0).ln(),
            Transfer::LogLog => (1.0 + (1.0 + x.max(0.0)).ln()).ln(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slopes {
    pub power: f64,
    /// Strength in percent.
    pub ratio: f64,
    pub angle_deg: f64,
}

impl Default for Slopes {
    fn default() -> Self {
        Slopes { power: 50.0, ratio: 20.0, angle_deg: 45.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Colouring {
    pub palette: Palette,
    /// Iterations per palette entry (`IterDiv`); the palette repeats
    /// every `1024 * iter_div` (transferred) iterations.
    pub iter_div: f64,
    /// Palette offset in entries, 0..1024 (`ColorOffset`).
    pub offset: f64,
    pub transfer: Transfer,
    /// Interpolate between palette entries (`Smooth`).
    pub smooth: bool,
    /// Drop the fractional iteration count (`Flat`).
    pub flat: bool,
    pub interior: [u8; 3],
    pub slopes: Option<Slopes>,
}

impl Default for Colouring {
    fn default() -> Self {
        Colouring {
            palette: Palette::classic(),
            iter_div: 0.1,
            offset: 0.0,
            transfer: Transfer::Linear,
            smooth: true,
            flat: false,
            interior: [0, 0, 0],
            slopes: Some(Slopes::default()),
        }
    }
}

impl Colouring {
    /// Colour a frame into 8-bit RGBA with an ordered dither.
    pub fn apply(&self, frame: &Frame) -> Vec<u8> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let mut out = vec![0u8; w * h * 4];
        let interior = self.interior.map(|c| c as f32 / 255.0);
        let mu = |x: usize, y: usize| frame.mu(y * w + x);
        let slopes = self.slopes.map(|s| {
            let (sin, cos) = s.angle_deg.to_radians().sin_cos();
            (s, sin, cos, s.power * w as f64 / 640.0)
        });
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let i = y * w + x;
                let mut rgb = if !frame.escaped(i) {
                    interior
                } else {
                    let m = frame.n[i] as f64 + if self.flat { 0.0 } else { frame.frac[i] as f64 };
                    let index = self.transfer.apply(m) / self.iter_div + self.offset;
                    self.palette.sample(index, self.smooth)
                };
                if let (Some((s, sin, cos, gain)), Some(p)) = (slopes, mu(x, y)) {
                    // Differences with the left and upper neighbours (or right/lower at the edges).
                    let diff = |a: Option<f64>, b: Option<f64>| match (a, b) {
                        (Some(a), Some(b)) => a - b,
                        _ => 0.0,
                    };
                    let dx = if x > 0 {
                        diff(mu(x - 1, y), Some(p))
                    } else if w > 1 {
                        diff(Some(p), mu(x + 1, y))
                    } else {
                        0.0
                    };
                    let dy = if y > 0 {
                        diff(mu(x, y - 1), Some(p))
                    } else if h > 1 {
                        diff(Some(p), mu(x, y + 1))
                    } else {
                        0.0
                    };
                    let d = (dx * cos + dy * sin) * gain;
                    let k = (d.abs().atan() / FRAC_PI_2 * s.ratio / 100.0) as f32;
                    for c in &mut rgb {
                        *c = if d >= 0.0 { (1.0 - k) * *c } else { (1.0 - k) * *c + k };
                    }
                }
                for c in 0..3 {
                    let mask = ((((x + c * 67) + y * 236) * 119) & 255) as f32 / 256.0;
                    row[x * 4 + c] = (255.0 * rgb[c] + mask).clamp(0.0, 255.0) as u8;
                }
                row[x * 4 + 3] = 255;
            }
        });
        out
    }
}
