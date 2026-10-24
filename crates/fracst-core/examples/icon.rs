//! Renders the fracst icon with fracst itself:
//! `cargo run --release -p fracst-core --example icon [-- <output dir>]`
//!
//! Writes `icon.png` (512 px), `icon.ico` (16–256 px, for the Windows
//! executable) and `icon.svg` (the PNG wrapped for tools that want SVG) to
//! the output directory, `assets/` by default.

use fracst_core::{Fixed, FloatExp, INTERIOR, Progress, Renderer, Settings, View, image};
use std::path::PathBuf;

/// Render size; every output size divides it, so downscaling is exact box filtering.
const BASE: u32 = 1536;
const ICO_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "assets".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let rgba = render();
    let png_at = |size: u32| image::encode_png(&image::downscale(&rgba, BASE, BASE, BASE / size), size, size).unwrap();

    std::fs::write(dir.join("icon.png"), png_at(512)).unwrap();
    let ico_pngs: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().filter(|&&s| s <= 256).map(|&s| (s, png_at(s))).collect();
    std::fs::write(dir.join("icon.ico"), ico(&ico_pngs)).unwrap();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">\
         <image width=\"64\" height=\"64\" href=\"data:image/png;base64,{}\"/></svg>\n",
        base64(&png_at(128))
    );
    std::fs::write(dir.join("icon.svg"), svg).unwrap();
    println!("wrote icon.png, icon.ico and icon.svg to {}", dir.display());
}

fn render() -> Vec<u8> {
    let view = View {
        re: Fixed::from_f64(-0.745, 3),
        im: Fixed::zero(3),
        zoom: FloatExp::from(2.0 / 1.42),
        maxiter: 4000,
        imag_down: false,
    };
    let settings = Settings { width: BASE, height: BASE, distance_estimate: true, ..Settings::default() };
    let frame = Renderer::new().render(&view, &settings, &Progress::default()).expect("render");

    let mut out = vec![0u8; (BASE * BASE * 4) as usize];
    let n = BASE as f64;
    for y in 0..BASE {
        for x in 0..BASE {
            let i = (y * BASE + x) as usize;
            // Normalised position, for the background gradient and the mask.
            let (u, v) = ((x as f64 + 0.5) / n, (y as f64 + 0.5) / n);
            let rgb = if frame.n[i] == INTERIOR {
                INTERIOR_COLOUR
            } else {
                // Background: deep indigo with a soft violet bloom behind the set,
                // lifted a little in the outer iteration bands.
                let mu = frame.n[i] as f64 + frame.frac[i] as f64;
                let bands = ((mu.ln() - 0.5) / 2.6).clamp(0.0, 1.0);
                let bloom = (-((u - 0.55).powi(2) + (v - 0.5).powi(2)) / 0.09).exp();
                let sky = mix(SPACE_TOP, SPACE_BOTTOM, v);
                let base = mix(sky, VIOLET, (0.5 * bloom + 0.2 * bands).min(1.0));
                // Glow: distance to the set, as a fraction of the icon size.
                let d = (frame.de[i] as f64 / n).max(0.0);
                let g = (-d / 0.034).exp();
                let glow = gradient(&[(0.0, MAGENTA), (0.5, ORANGE), (0.82, GOLD), (1.0, WHITE)], g.powf(1.4));
                mix(base, glow, (1.3 * g.powf(0.9)).min(1.0))
            };
            let alpha = rounded_square(u, v, 0.225, 1.0 / n);
            let px = &mut out[i * 4..i * 4 + 4];
            for c in 0..3 {
                px[c] = (rgb[c] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            px[3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}

const INTERIOR_COLOUR: [f64; 3] = [0.035, 0.03, 0.08];
const SPACE_TOP: [f64; 3] = [0.07, 0.06, 0.20];
const SPACE_BOTTOM: [f64; 3] = [0.03, 0.04, 0.13];
const VIOLET: [f64; 3] = [0.30, 0.14, 0.55];
const MAGENTA: [f64; 3] = [0.86, 0.22, 0.60];
const ORANGE: [f64; 3] = [1.0, 0.55, 0.22];
const GOLD: [f64; 3] = [1.0, 0.86, 0.52];
const WHITE: [f64; 3] = [1.0, 0.98, 0.93];

fn mix(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

fn gradient(stops: &[(f64, [f64; 3])], t: f64) -> [f64; 3] {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (w[0], w[1]);
        if t <= t1 {
            return mix(c0, c1, (t - t0) / (t1 - t0));
        }
    }
    stops.last().unwrap().1
}

/// Coverage of a rounded square (corner radius `r`, unit coordinates) at `(u, v)`,
/// antialiased over `aa`.
fn rounded_square(u: f64, v: f64, r: f64, aa: f64) -> f64 {
    let qx = (u - 0.5).abs() - (0.5 - r);
    let qy = (v - 0.5).abs() - (0.5 - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
    (0.5 - outside / aa).clamp(0.0, 1.0)
}

/// A Windows icon file holding PNG-compressed images.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                s.push(T[(v >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}
