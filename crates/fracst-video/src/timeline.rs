//! Where the video is at each moment: the zoom (from the speed curve) and
//! the colours (palette cycling and colour keys).

use crate::project::{ColourAnim, ColourKey, Length, Plan, Timing};
use fracst_core::{Colouring, Slopes};

/// The zoom over time, resolved for a video's length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    /// The whole video, in seconds.
    pub total: f64,
    hold_start: f64,
    /// Seconds spent zooming, between the holds.
    zooming: f64,
    ease_in: f64,
    ease_out: f64,
    /// `log2(zoom / end_zoom)` at the start (negative).
    start_log2: f64,
}

impl Timeline {
    /// `audio` is the audio track's length, needed for [`Length::FitAudio`].
    pub fn new(timing: &Timing, plan: &Plan, audio: Option<f64>) -> Result<Timeline, String> {
        let holds = timing.hold_start + timing.hold_end;
        let (total, zooming, ease_in, ease_out) = match timing.length {
            Length::PerDoubling(s) => {
                // At full speed for the whole way, plus what the ramps lose.
                let zooming = plan.doublings() * s + (timing.ease_in + timing.ease_out) / 2.0;
                (zooming + holds, zooming, timing.ease_in, timing.ease_out)
            }
            Length::Seconds(total) => (total, total - holds, timing.ease_in, timing.ease_out),
            Length::FitAudio => {
                let total = audio.ok_or("fitting the length to the audio needs the audio track's length")?;
                (total, total - holds, timing.ease_in, timing.ease_out)
            }
        };
        if !(zooming > 0.0) {
            return Err(format!("a {total:.1} s video leaves no time to zoom after holding for {holds:.1} s"));
        }
        // Ramps longer than the zoom itself are shortened in proportion.
        let ramps = ease_in + ease_out;
        let k = if ramps > zooming { zooming / ramps } else { 1.0 };
        Ok(Timeline {
            total,
            hold_start: timing.hold_start,
            zooming,
            ease_in: ease_in * k,
            ease_out: ease_out * k,
            start_log2: plan.start_log2,
        })
    }

    pub fn frames(&self, fps: f64) -> usize {
        ((self.total * fps).round() as usize).max(1)
    }

    /// Fraction of the way from the start zoom to the end, in `[0, 1]`
    /// (in zoom doublings), `t` seconds into the video.
    pub fn progress(&self, t: f64) -> f64 {
        let tau = (t - self.hold_start).clamp(0.0, self.zooming);
        let (a, b, z) = (self.ease_in, self.ease_out, self.zooming);
        // Speed rises as (1 - cos)/2 over `a` seconds and falls likewise over
        // the last `b`; the distance covered is its integral.
        let ramp = |x: f64, len: f64| {
            if len <= 0.0 {
                x
            } else {
                x / 2.0 - len / (2.0 * std::f64::consts::PI) * (std::f64::consts::PI * x / len).sin()
            }
        };
        let full = z - a / 2.0 - b / 2.0;
        let covered = if tau < a {
            ramp(tau, a)
        } else if tau <= z - b {
            a / 2.0 + (tau - a)
        } else {
            full - ramp(z - tau, b)
        };
        (covered / full).clamp(0.0, 1.0)
    }

    /// `log2(zoom / end_zoom)` at `t` seconds: from the start (negative) to 0.
    pub fn zoom_log2(&self, t: f64) -> f64 {
        self.start_log2 * (1.0 - self.progress(t))
    }
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A field's value at `t` from the keys that set it (sorted by time), eased
/// between neighbouring keys. `geometric` interpolates ratios, for `IterDiv`.
fn field(keys: &[ColourKey], t: f64, get: impl Fn(&ColourKey) -> Option<f64>, geometric: bool) -> Option<f64> {
    let set: Vec<(f64, f64)> = keys.iter().filter_map(|k| get(k).map(|v| (k.time, v))).collect();
    let (first, last) = (set.first()?, set.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    let i = set.iter().position(|(time, _)| *time > t)?;
    let ((t0, v0), (t1, v1)) = (set[i - 1], set[i]);
    let u = if t1 > t0 { smoothstep((t - t0) / (t1 - t0)) } else { 1.0 };
    Some(if geometric && v0 > 0.0 && v1 > 0.0 { v0 * (v1 / v0).powf(u) } else { v0 + (v1 - v0) * u })
}

/// The colouring `t` seconds into the video.
pub fn colouring_at(base: &Colouring, anim: &ColourAnim, t: f64) -> Colouring {
    let mut c = base.clone();
    let mut keys = anim.keys.clone();
    keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    if let Some(v) = field(&keys, t, |k| k.offset, false) {
        c.offset = v;
    }
    if let Some(v) = field(&keys, t, |k| k.iter_div, true) {
        c.iter_div = v.max(1e-9);
    }
    let power = field(&keys, t, |k| k.slope_power, false);
    let ratio = field(&keys, t, |k| k.slope_ratio, false);
    let angle = field(&keys, t, |k| k.slope_angle, false);
    if power.is_some() || ratio.is_some() || angle.is_some() {
        let s = c.slopes.get_or_insert_with(Slopes::default);
        s.power = power.unwrap_or(s.power).max(0.0);
        s.ratio = ratio.unwrap_or(s.ratio).clamp(0.0, 100.0);
        s.angle_deg = angle.unwrap_or(s.angle_deg);
    }
    c.offset = (c.offset + anim.cycle * t).rem_euclid(1024.0);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::KeyframeSpec;
    use fracst_core::FloatExp;

    fn plan() -> Plan {
        Plan::new(FloatExp::exp2(50.0), &KeyframeSpec::default()).unwrap()
    }

    #[test]
    fn speed_curve() {
        let timing =
            Timing { length: Length::PerDoubling(1.0), ease_in: 2.0, ease_out: 4.0, hold_start: 1.0, hold_end: 2.0 };
        let tl = Timeline::new(&timing, &plan(), None).unwrap();
        // 50 doublings at one per second, plus half of each ramp, plus holds.
        assert!((tl.total - (50.0 + 3.0 + 3.0)).abs() < 1e-9);
        assert_eq!(tl.zoom_log2(0.0), -50.0);
        assert_eq!(tl.zoom_log2(1.0), -50.0);
        assert_eq!(tl.zoom_log2(tl.total), 0.0);
        assert_eq!(tl.zoom_log2(tl.total - 1.0), 0.0);
        // Monotonic, and one doubling per second at full speed.
        let mut last = f64::NEG_INFINITY;
        for i in 0..=1000 {
            let z = tl.zoom_log2(tl.total * i as f64 / 1000.0);
            assert!(z >= last - 1e-12);
            last = z;
        }
        assert!((tl.zoom_log2(20.0) - tl.zoom_log2(19.0) - 1.0).abs() < 1e-9);
        // Continuous where the ramps meet the constant part.
        let around = |t: f64| (tl.zoom_log2(t + 1e-6) - tl.zoom_log2(t - 1e-6)).abs();
        assert!(around(3.0) < 1e-5 && around(tl.total - 6.0) < 1e-5);
    }

    #[test]
    fn fixed_length_and_audio() {
        let timing = Timing { length: Length::Seconds(30.0), ..Timing::default() };
        let tl = Timeline::new(&timing, &plan(), None).unwrap();
        assert_eq!(tl.total, 30.0);
        assert_eq!(tl.frames(60.0), 1800);
        let timing = Timing { length: Length::FitAudio, ..Timing::default() };
        assert!(Timeline::new(&timing, &plan(), None).is_err());
        assert_eq!(Timeline::new(&timing, &plan(), Some(12.5)).unwrap().total, 12.5);
        let timing = Timing { length: Length::Seconds(1.0), hold_end: 2.0, ..Timing::default() };
        assert!(Timeline::new(&timing, &plan(), None).is_err());
    }

    #[test]
    fn colour_keys() {
        let base = Colouring::default();
        let anim = ColourAnim {
            cycle: 10.0,
            keys: vec![
                ColourKey { time: 10.0, iter_div: Some(1.0), ..Default::default() },
                ColourKey { time: 0.0, offset: Some(100.0), iter_div: Some(0.01), ..Default::default() },
            ],
        };
        let c = colouring_at(&base, &anim, 0.0);
        assert_eq!((c.offset, c.iter_div), (100.0, 0.01));
        let c = colouring_at(&base, &anim, 5.0);
        assert!((c.iter_div - 0.1).abs() < 1e-12, "geometric midpoint");
        assert_eq!(c.offset, 150.0, "offset key held, plus cycling");
        let c = colouring_at(&base, &anim, 20.0);
        assert_eq!(c.iter_div, 1.0);
        assert_eq!(colouring_at(&base, &ColourAnim::default(), 7.0), base);
    }
}
