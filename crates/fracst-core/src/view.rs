//! Image location: a high-precision centre plus an extended-range zoom.

use crate::fixed::{Fixed, limbs_for_bits};
use crate::floatexp::FloatExp;

/// Where to look and how deep. Follows the KF2 convention: at zoom `z` the
/// image height spans `4 / z`, i.e. half the height is `2 / z`.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub re: Fixed,
    pub im: Fixed,
    pub zoom: FloatExp,
    pub maxiter: u64,
    /// KF2 compatibility: imaginary axis increases downwards (KF2's default).
    pub imag_down: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            re: Fixed::from_f64(-0.75, 3),
            im: Fixed::zero(3),
            zoom: FloatExp::from(1.0),
            maxiter: 1000,
            imag_down: false,
        }
    }
}

impl View {
    /// Parse a location from decimal strings, choosing a precision suitable
    /// for the zoom.
    pub fn parse(re: &str, im: &str, zoom: &str, maxiter: u64) -> Option<View> {
        let zoom = FloatExp::parse(zoom)?;
        if !(zoom > FloatExp::ZERO) {
            return None;
        }
        let n = Self::limbs_for(zoom, 4096);
        Some(View { re: Fixed::parse(re, n)?, im: Fixed::parse(im, n)?, zoom, maxiter, imag_down: false })
    }

    /// Limbs needed to place pixels accurately at this zoom and image height.
    pub fn limbs_for(zoom: FloatExp, height: u32) -> usize {
        let bits = (zoom.log2() + (height.max(1) as f64).log2()).max(0.0);
        limbs_for_bits(bits as u64 + 16)
    }

    /// Grow the centre's precision if the current zoom needs more.
    pub fn ensure_precision(&mut self, height: u32) {
        let n = Self::limbs_for(self.zoom, height);
        if self.re.limbs() < n {
            self.re = self.re.with_limbs(n);
            self.im = self.im.with_limbs(n);
        }
    }

    pub fn pixel_spacing(&self, height: u32) -> FloatExp {
        FloatExp::from(4.0) / (self.zoom * FloatExp::from(height.max(1) as f64))
    }

    /// Offset in pixels from the image centre (real right, imaginary up/down)
    /// for pixel coordinates `(x, y)` measured from the top-left corner.
    pub fn pixel_offset(&self, x: f64, y: f64, width: u32, height: u32) -> (f64, f64) {
        let dx = x - width as f64 / 2.0;
        let dy = y - height as f64 / 2.0;
        (dx, if self.imag_down { dy } else { -dy })
    }

    /// Move the centre by a pixel offset.
    pub fn translate_pixels(&mut self, dx: f64, dy_up: f64, height: u32) {
        let s = self.pixel_spacing(height);
        let n = self.re.limbs();
        self.re = self.re.add(&Fixed::from_floatexp(s * FloatExp::from(dx), n));
        self.im = self.im.add(&Fixed::from_floatexp(s * FloatExp::from(dy_up), n));
    }

    /// Zoom by `factor` keeping the point under pixel `(x, y)` fixed.
    pub fn zoom_at(&mut self, x: f64, y: f64, width: u32, height: u32, factor: f64) {
        let (dx, dy) = self.pixel_offset(x, y, width, height);
        let k = 1.0 - 1.0 / factor;
        self.translate_pixels(dx * k, dy * k, height);
        self.zoom = self.zoom * FloatExp::from(factor);
        self.ensure_precision(height);
    }

    /// Decimal digits worth printing for the centre at this zoom.
    pub fn digits(&self) -> usize {
        (20.0 + self.zoom.log2().max(0.0) * std::f64::consts::LOG10_2) as usize
    }

    pub fn re_string(&self) -> String {
        self.re.to_decimal(self.digits())
    }

    pub fn im_string(&self) -> String {
        self.im.to_decimal(self.digits())
    }
}
