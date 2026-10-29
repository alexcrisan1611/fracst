//! Animated zooms. The view the app renders (the *target*) changes at once;
//! what is displayed eases towards it. The display is the target zoomed out
//! by `k` and shifted, so the frame on screen scales and slides into place
//! while the new one renders.
//!
//! Offsets are in screen pixels from the centre, y down.

use std::time::{Duration, Instant};

/// How far the display is from the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offset {
    /// The display is the target zoomed out by this factor.
    pub k: f64,
    /// The display's centre, relative to the target's, in target pixels.
    pub d: [f64; 2],
}

impl Offset {
    pub const NONE: Offset = Offset { k: 1.0, d: [0.0, 0.0] };

    /// Where a point shown at `u` (display pixels from the centre) is, in target pixels.
    pub fn to_target(self, u: [f64; 2]) -> [f64; 2] {
        [u[0] * self.k + self.d[0], u[1] * self.k + self.d[1]]
    }

    /// Where something at `c` (target pixels from the centre) is displayed,
    /// in display pixels, and how much it is scaled.
    pub fn to_display(self, c: [f64; 2]) -> ([f64; 2], f64) {
        ([(c[0] - self.d[0]) / self.k, (c[1] - self.d[1]) / self.k], 1.0 / self.k)
    }

    /// The same display, relative to a new target. `ratio` is the new
    /// target's zoom over the old one's; `centre` is where the old target's
    /// centre is in the new target's pixels.
    pub fn retarget(self, ratio: f64, centre: [f64; 2]) -> Offset {
        Offset { k: self.k * ratio, d: [centre[0] + self.d[0] * ratio, centre[1] + self.d[1] * ratio] }
    }
}

/// An offset easing to nothing.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    /// log2 of the zoom-out at the start.
    a: f64,
    /// Where the target's centre appears at the start, in display pixels.
    s: [f64; 2],
    start: Instant,
}

impl Glide {
    /// Start easing from `from` to nothing. The target's centre slides in a
    /// straight line on screen while the zoom changes at an even pace.
    pub fn new(from: Offset, start: Instant) -> Glide {
        Glide { a: from.k.log2(), s: [from.d[0] / from.k, from.d[1] / from.k], start }
    }

    /// The offset `progress` (0 to 1) of the way through.
    pub fn at(&self, progress: f64) -> Offset {
        let rest = 1.0 - ease(progress.clamp(0.0, 1.0));
        let k = (self.a * rest).exp2();
        Offset { k, d: [self.s[0] * rest * k, self.s[1] * rest * k] }
    }

    pub fn progress(&self, now: Instant, length: Duration) -> f64 {
        if length.is_zero() {
            1.0
        } else {
            now.saturating_duration_since(self.start).as_secs_f64() / length.as_secs_f64()
        }
    }
}

/// Fast at first, settling gently.
pub fn ease(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn click_zoom_starts_where_it_was_and_ends_on_target() {
        // Clicking at p zooms ×4 into it: the new target is centred on p.
        let p = [120.0, -40.0];
        let o = Offset::NONE.retarget(4.0, [-p[0] * 4.0, -p[1] * 4.0]);
        // At first the display is the old view: the new centre shows at p.
        let g = Glide::new(o, Instant::now());
        let (c, scale) = g.at(0.0).to_display([0.0, 0.0]);
        assert!(close(c, p), "{c:?}");
        assert!((scale - 0.25).abs() < 1e-12);
        // Half way it has slid part of the way, and is zoomed in part of the way.
        let (c, scale) = g.at(0.5).to_display([0.0, 0.0]);
        assert!(c[0] > 0.0 && c[0] < p[0] && c[1] < 0.0 && c[1] > p[1], "{c:?}");
        assert!(scale > 0.25 && scale < 1.0);
        // At the end the display is the target.
        assert_eq!(g.at(1.0), Offset::NONE);
    }

    #[test]
    fn retargeting_keeps_the_display_still() {
        // Mid-glide, a second zoom must not move what is on screen.
        let o = Glide::new(Offset::NONE.retarget(4.0, [80.0, 20.0]), Instant::now()).at(0.3);
        let (ratio, centre) = (2.0, [-35.0, 12.0]);
        let n = o.retarget(ratio, centre);
        for u in [[0.0, 0.0], [100.0, -50.0], [-300.0, 200.0]] {
            // The same point on screen is the same place, in the new target's pixels.
            let old = o.to_target(u);
            let old_in_new = [centre[0] + old[0] * ratio, centre[1] + old[1] * ratio];
            assert!(close(n.to_target(u), old_in_new));
        }
        // And a glide from there starts exactly there.
        let g = Glide::new(n, Instant::now()).at(0.0);
        assert!((g.k - n.k).abs() < 1e-9 && close(g.d, n.d));
    }

    #[test]
    fn progress_handles_zero_length() {
        let g = Glide::new(Offset::NONE, Instant::now());
        assert_eq!(g.progress(Instant::now(), Duration::ZERO), 1.0);
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
    }
}
