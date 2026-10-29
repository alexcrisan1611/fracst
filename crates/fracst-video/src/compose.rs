//! Building video frames from keyframes.
//!
//! Every keyframe shares the location's centre, keyframe `k` being `factor^k`
//! times shallower than the location. A frame at zoom `z` stacks the
//! keyframes whose zoom `Z_k` lies within `factor^2` either side of `z`:
//! scaled to the frame, the shallowest covers it entirely, and each deeper
//! one covers a smaller centred part in more detail. Layers are drawn from
//! the outside in, each over the last, fading out over a margin at its
//! edges, and the deepest fade in as they come into range, so nothing
//! jumps as the zoom moves from one keyframe to the next.
//!
//! Each layer is coloured at its own resolution (the colours can change
//! from frame to frame), then sampled with a mipmap so that shrunken
//! layers do not alias, `taps x taps` times per output pixel. Slope
//! shading is applied after sampling, scaled to output pixels, so that
//! layers of different scales shade alike.

use crate::keyframe::Keyframe;
use crate::project::{Plan, Project};
use crate::timeline::{Timeline, colouring_at};
use fracst_core::{Colouring, Frame};
use rayon::prelude::*;
use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

/// Width of the faded border of each layer, as a fraction of its height.
const EDGE: f64 = 0.04;
/// Deepest mip level kept.
const MAX_LEVELS: usize = 8;

/// What a mipmap can hold: up to four channels, averaged separately.
trait Texel: Copy + Send + Sync {
    fn channels(&self) -> [f32; 4];
    fn from_channels(v: [f32; 4]) -> Self;
}

impl Texel for [u8; 4] {
    #[inline]
    fn channels(&self) -> [f32; 4] {
        self.map(|c| c as f32)
    }

    fn from_channels(v: [f32; 4]) -> Self {
        v.map(|c| c.round().clamp(0.0, 255.0) as u8)
    }
}

/// A pixel's slope: differences of the smooth iteration count with its
/// neighbours across and down, as bfloat16s (plenty for shading).
#[derive(Clone, Copy, Default)]
struct Slope([u16; 2]);

fn to_bf16(x: f32) -> u16 {
    let x = if x.is_finite() { x } else { 0.0 };
    (x.to_bits().wrapping_add(0x8000) >> 16) as u16
}

impl Texel for Slope {
    #[inline]
    fn channels(&self) -> [f32; 4] {
        let f = |b: u16| f32::from_bits((b as u32) << 16);
        [f(self.0[0]), f(self.0[1]), 0.0, 0.0]
    }

    fn from_channels(v: [f32; 4]) -> Self {
        Slope([to_bf16(v[0]), to_bf16(v[1])])
    }
}

/// An image and its successive halvings.
struct Pyramid<T> {
    levels: Vec<(Vec<T>, usize, usize)>,
}

impl<T: Texel> Pyramid<T> {
    fn new(base: Vec<T>, w: usize, h: usize, levels: usize) -> Self {
        let mut out = vec![(base, w, h)];
        while out.len() < levels.clamp(1, MAX_LEVELS) {
            let (src, sw, sh) = out.last().unwrap();
            if *sw < 2 || *sh < 2 {
                break;
            }
            let (sw, dw, dh) = (*sw, sw / 2, sh / 2);
            let dst: Vec<T> = (0..dw * dh)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = (2 * (i % dw), 2 * (i / dw));
                    let at = |x: usize, y: usize| src[y * sw + x].channels();
                    let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
                    T::from_channels(std::array::from_fn(|k| (a[k] + b[k] + c[k] + d[k]) / 4.0))
                })
                .collect();
            out.push((dst, dw, dh));
        }
        Pyramid { levels: out }
    }

    /// Bilinear sample of `level` at `(x, y)` in level-0 pixel units.
    #[inline]
    fn sample(&self, level: usize, x: f64, y: f64) -> [f32; 4] {
        let level = level.min(self.levels.len() - 1);
        let (data, w, h) = &self.levels[level];
        let scale = 1.0 / (1u64 << level) as f64;
        let u = (x * scale - 0.5).clamp(0.0, (*w - 1) as f64);
        let v = (y * scale - 0.5).clamp(0.0, (*h - 1) as f64);
        let (x0, y0) = (u as usize, v as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
        let (a, b, c, d) = (
            data[y0 * w + x0].channels(),
            data[y0 * w + x1].channels(),
            data[y1 * w + x0].channels(),
            data[y1 * w + x1].channels(),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }

    /// Trilinear sample: `lod` is log2 of level-0 pixels per sample spacing.
    #[inline]
    fn sample_lod(&self, lod: f64, x: f64, y: f64) -> [f32; 4] {
        let lod = lod.max(0.0);
        let l0 = lod.floor() as usize;
        let t = (lod - l0 as f64) as f32;
        let a = self.sample(l0, x, y);
        if t < 1e-3 || l0 + 1 >= self.levels.len() {
            return a;
        }
        let b = self.sample(l0 + 1, x, y);
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
    }
}

/// The slopes of a frame, from the neighbours `Colouring::apply` uses.
fn slopes_of(frame: &Frame) -> Vec<Slope> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mu = |x: usize, y: usize| frame.mu(y * w + x);
    let mut out = vec![Slope::default(); w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, s) in row.iter_mut().enumerate() {
            let Some(p) = mu(x, y) else { continue };
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
            *s = Slope([to_bf16(dx as f32), to_bf16(dy as f32)]);
        }
    });
    out
}

/// One layer of a frame: where it sits and how strongly it shows.
struct Layer {
    colours: Arc<Pyramid<[u8; 4]>>,
    slopes: Option<Arc<Pyramid<Slope>>>,
    /// Layer pixels per output pixel.
    scale: f64,
    width: f64,
    height: f64,
    /// Fades in deep layers as they come into range.
    weight: f32,
    /// The outermost layer covers everything, without a faded border.
    bottom: bool,
}

/// Slope shading for a frame, matching what `Colouring::apply` gives a
/// frame rendered at the output size, whatever each layer's own scale.
#[derive(Clone, Copy)]
struct Shading {
    cos: f64,
    sin: f64,
    /// Gain per output pixel.
    gain: f64,
    ratio: f64,
}

/// A layer's colours (without slopes) and the colouring they were made with.
type Coloured = (Colouring, Arc<Pyramid<[u8; 4]>>);

/// Turns keyframes into video frames, keeping decoded and coloured
/// keyframes while they are in use.
pub struct Compositor<'a> {
    project: &'a Project,
    plan: Plan,
    decoded: HashMap<usize, Arc<Keyframe>>,
    slopes: HashMap<usize, Arc<Pyramid<Slope>>>,
    coloured: HashMap<usize, Coloured>,
}

/// `log2` of how much deeper keyframe `k` is than a frame at `zoom_log2`.
fn depth(plan: &Plan, k: usize, zoom_log2: f64) -> f64 {
    plan.log2(k) - zoom_log2
}

impl<'a> Compositor<'a> {
    pub fn new(project: &'a Project) -> Self {
        Compositor {
            project,
            plan: project.plan(),
            decoded: HashMap::new(),
            slopes: HashMap::new(),
            coloured: HashMap::new(),
        }
    }

    /// Keyframes used by a frame at `zoom_log2` (relative to the location).
    pub fn layers_for(&self, zoom_log2: f64) -> Vec<usize> {
        let f = self.plan.factor.log2();
        (0..self.plan.count)
            .filter(|&k| {
                let d = depth(&self.plan, k, zoom_log2);
                d > -2.0 * f + 1e-9 && d <= 2.0 * f
            })
            .collect()
    }

    /// The frame `t` seconds into the video, `width x height` RGBA. Missing
    /// keyframes are left out (black where nothing covers).
    pub fn frame(
        &mut self,
        timeline: &Timeline,
        t: f64,
        width: u32,
        height: u32,
        taps: u32,
    ) -> Result<Vec<u8>, String> {
        let colouring = colouring_at(&self.project.location.colouring, &self.project.spec.colour, t);
        self.frame_at(timeline.zoom_log2(t), &colouring, width, height, taps)
    }

    pub fn frame_at(
        &mut self,
        zoom_log2: f64,
        colouring: &Colouring,
        width: u32,
        height: u32,
        taps: u32,
    ) -> Result<Vec<u8>, String> {
        let wanted = self.layers_for(zoom_log2);
        self.decoded.retain(|k, _| wanted.contains(k));
        self.slopes.retain(|k, _| wanted.contains(k));
        self.coloured.retain(|k, _| wanted.contains(k));
        let f = self.plan.factor.log2();
        let taps = taps.max(1);
        let flat = Colouring { slopes: None, ..colouring.clone() };
        let mut layers = Vec::new();
        // From the outside in: the deepest last, so it ends up on top.
        for &k in wanted.iter().rev() {
            if !self.project.has_keyframe(k) {
                continue;
            }
            if !self.decoded.contains_key(&k) {
                self.decoded.insert(k, Arc::new(self.project.load_keyframe(k)?));
            }
            let kf = self.decoded[&k].clone();
            let (kw, kh) = (kf.frame.width as usize, kf.frame.height as usize);
            let d = depth(&self.plan, k, zoom_log2);
            let size = kh as f64 / height as f64;
            let scale = d.exp2() * size;
            // Mip levels down to one layer pixel per sample, as deep as a layer goes.
            let levels = ((2.0 * f).exp2() * size / taps as f64).log2().max(0.0).ceil() as usize + 2;
            let colours = match self.coloured.get(&k) {
                Some((c, m)) if *c == flat => m.clone(),
                _ => {
                    let rgba = flat.apply(&kf.frame);
                    let texels = rgba.as_chunks::<4>().0.to_vec();
                    let m = Arc::new(Pyramid::new(texels, kw, kh, levels));
                    self.coloured.insert(k, (flat.clone(), m.clone()));
                    m
                }
            };
            let slopes = colouring.slopes.map(|_| {
                let field = || Arc::new(Pyramid::new(slopes_of(&kf.frame), kw, kh, levels));
                self.slopes.entry(k).or_insert_with(field).clone()
            });
            let weight = if d <= f { 1.0 } else { (1.0 - (d - f) / f).clamp(0.0, 1.0) };
            layers.push(Layer {
                colours,
                slopes,
                scale,
                width: kw as f64,
                height: kh as f64,
                weight: (weight * weight * (3.0 - 2.0 * weight)) as f32,
                bottom: false,
            });
        }
        if let Some(first) = layers.first_mut() {
            first.bottom = true;
        }
        let shading = colouring.slopes.map(|s| {
            let (sin, cos) = s.angle_deg.to_radians().sin_cos();
            Shading { cos, sin, gain: s.power * width as f64 / 640.0, ratio: s.ratio / 100.0 }
        });
        Ok(composite(&layers, shading, width, height, taps))
    }
}

fn composite(layers: &[Layer], shading: Option<Shading>, width: u32, height: u32, taps: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    let n = taps as usize;
    let lods: Vec<f64> = layers.iter().map(|l| (l.scale / n as f64).log2()).collect();
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut sum = [0f32; 3];
            for j in 0..n {
                for i in 0..n {
                    // Offset from the frame centre in output pixels.
                    let dx = x as f64 + (i as f64 + 0.5) / n as f64 - w as f64 / 2.0;
                    let dy = y as f64 + (j as f64 + 0.5) / n as f64 - h as f64 / 2.0;
                    let mut c = [0f32; 3];
                    for (layer, &lod) in layers.iter().zip(&lods) {
                        let lx = layer.width / 2.0 + dx * layer.scale;
                        let ly = layer.height / 2.0 + dy * layer.scale;
                        let alpha = if layer.bottom {
                            1.0
                        } else {
                            let edge = lx.min(layer.width - lx).min(ly).min(layer.height - ly);
                            if edge <= 0.0 {
                                continue;
                            }
                            let e = (edge / (EDGE * layer.height)).min(1.0) as f32;
                            e * e * (3.0 - 2.0 * e) * layer.weight
                        };
                        if alpha <= 0.0 {
                            continue;
                        }
                        let mut s = layer.colours.sample_lod(lod, lx, ly);
                        if let (Some(sh), Some(slopes)) = (shading, &layer.slopes) {
                            // Differences per layer pixel, scaled to output pixels.
                            let [sx, sy, ..] = slopes.sample_lod(lod, lx, ly);
                            let d = (sx as f64 * sh.cos + sy as f64 * sh.sin) * sh.gain * layer.scale;
                            let k = (d.abs().atan() / FRAC_PI_2 * sh.ratio) as f32;
                            for v in &mut s[..3] {
                                *v = if d >= 0.0 { (1.0 - k) * *v } else { (1.0 - k) * *v + k * 255.0 };
                            }
                        }
                        for k in 0..3 {
                            c[k] += (s[k] - c[k]) * alpha;
                        }
                    }
                    for k in 0..3 {
                        sum[k] += c[k];
                    }
                }
            }
            let inv = 1.0 / (n * n) as f32;
            for k in 0..3 {
                row[x * 4 + k] = (sum[k] * inv).round().clamp(0.0, 255.0) as u8;
            }
            row[x * 4 + 3] = 255;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pyramid(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Pyramid<[u8; 4]> {
        let texels = (0..w * h)
            .map(|i| {
                let v = f(i % w, i / w);
                [v, v, v, 255]
            })
            .collect();
        Pyramid::new(texels, w, h, 4)
    }

    #[test]
    fn mip_levels_average() {
        let m = pyramid(8, 8, |x, _| if x % 2 == 0 { 0 } else { 200 });
        assert_eq!(m.levels.len(), 4);
        assert_eq!(m.levels[1].0[0][0], 100);
        assert_eq!(m.sample(0, 0.5, 0.5)[0], 0.0);
        assert_eq!(m.sample(0, 1.5, 0.5)[0], 200.0);
        assert_eq!(m.sample(0, 1.0, 0.5)[0], 100.0);
        assert_eq!(Slope::from_channels([0.375, -2.0, 0.0, 0.0]).channels()[..2], [0.375, -2.0]);
    }

    fn layer(value: u8, scale: f64, bottom: bool) -> Layer {
        Layer {
            colours: Arc::new(pyramid(64, 64, |_, _| value)),
            slopes: None,
            scale,
            width: 64.0,
            height: 64.0,
            weight: 1.0,
            bottom,
        }
    }

    #[test]
    fn layers_stack_from_outside_in() {
        // A uniform bottom layer and a smaller, deeper one drawn over the centre.
        let img = composite(&[layer(50, 1.0, true), layer(250, 2.0, false)], None, 64, 64, 2);
        let at = |x: usize, y: usize| img[(y * 64 + x) * 4];
        assert_eq!(at(32, 32), 250, "the deeper layer shows in the centre");
        assert_eq!(at(2, 2), 50, "the outer layer shows outside it");
        let edge = at(16, 32);
        assert!(edge > 50 && edge < 250, "and they blend at its border ({edge})");
    }

    #[test]
    fn slopes_shade_alike_at_any_scale() {
        // The same smooth ramp seen by two layers, one at twice the
        // resolution (so half the difference per pixel): equal shading.
        let ramp = |step: f32| {
            let field = vec![Slope([to_bf16(step), 0]); 64 * 64];
            Some(Arc::new(Pyramid::new(field, 64, 64, 4)))
        };
        let shading = Some(Shading { cos: 1.0, sin: 0.0, gain: 0.5, ratio: 0.5 });
        let mut coarse = layer(200, 1.0, true);
        coarse.slopes = ramp(0.2);
        let mut fine = layer(200, 2.0, true);
        fine.slopes = ramp(0.1);
        let a = composite(&[coarse], shading, 16, 16, 1);
        let b = composite(&[fine], shading, 16, 16, 1);
        assert_eq!(a[0], b[0]);
        assert!(a[0] < 200, "shaded darker");
    }
}
