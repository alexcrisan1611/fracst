//! Kalles Fraktaler 2 location files (`.kfr`) and image metadata.
//!
//! KFR is `Key: value` per line. We read the location, iteration and colour
//! keys we support, ignore the rest, and write files KF2 can open.

use crate::colour::{Colouring, Palette, Slopes, Transfer};
use crate::fixed::Fixed;
use crate::view::View;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Location {
    pub view: View,
    pub colouring: Colouring,
    /// Image size, if the file records it.
    pub size: Option<(u32, u32)>,
}

pub fn parse(text: &str) -> Result<Location, String> {
    let kv: HashMap<&str, &str> =
        text.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim(), v.trim())).collect();
    let get = |k: &str| kv.get(k).copied();
    let num = |k: &str| get(k).and_then(|v| v.parse::<f64>().ok());
    let re = get("Re").ok_or("missing Re")?;
    let im = get("Im").ok_or("missing Im")?;
    let zoom = get("Zoom").ok_or("missing Zoom")?;
    let maxiter = num("Iterations").map_or(1000, |v| v.max(1.0) as u64);
    let mut view = View::parse(re, im, zoom, maxiter).ok_or("invalid Re/Im/Zoom")?;
    // Re-parse the centre with enough precision for every digit given.
    let digits = re.len().max(im.len()) as u64;
    let n = view.re.limbs().max(crate::fixed::limbs_for_bits((digits as f64 * 3.33) as u64));
    view.re = Fixed::parse(re, n).ok_or("invalid Re")?;
    view.im = Fixed::parse(im, n).ok_or("invalid Im")?;
    view.imag_down = num("ImagPointsUp").is_none_or(|v| v == 0.0);
    if num("Power").is_some_and(|p| p != 2.0) || num("FractalType").is_some_and(|t| t != 0.0) {
        return Err("only the standard Mandelbrot set (FractalType 0, Power 2) is supported".into());
    }

    let mut c = Colouring::default();
    if let Some(colors) = get("Colors") {
        // KF2 stores B,G,R triplets.
        let v: Vec<u8> = colors.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let keys: Vec<[u8; 3]> = v.as_chunks::<3>().0.iter().map(|t| [t[2], t[1], t[0]]).collect();
        if !keys.is_empty() {
            c.palette = Palette::new(keys);
        }
    }
    if let Some(v) = get("InteriorColor") {
        let v: Vec<u8> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() >= 3 {
            c.interior = [v[2], v[1], v[0]];
        }
    }
    if let Some(d) = num("IterDiv").filter(|d| *d > 0.0) {
        c.iter_div = d;
    }
    c.offset = num("ColorOffset").unwrap_or(0.0);
    c.transfer = Transfer::from_kf2_method(num("ColorMethod").unwrap_or(0.0) as u32);
    c.smooth = num("Smooth").is_none_or(|v| v != 0.0);
    c.flat = num("Flat").is_some_and(|v| v != 0.0);
    c.slopes = num("Slopes").filter(|v| *v != 0.0).map(|_| Slopes {
        power: num("SlopePower").unwrap_or(50.0),
        ratio: num("SlopeRatio").unwrap_or(20.0),
        angle_deg: num("SlopeAngle").unwrap_or(45.0),
    });
    let size = match (num("ImageWidth").or(num("WindowWidth")), num("ImageHeight").or(num("WindowHeight"))) {
        (Some(w), Some(h)) if w >= 1.0 && h >= 1.0 => Some((w as u32, h as u32)),
        _ => None,
    };
    Ok(Location { view, colouring: c, size })
}

pub fn to_string(view: &View, c: &Colouring) -> String {
    let mut s = String::new();
    let bgr = |rgb: &[u8; 3]| format!("{},{},{},", rgb[2], rgb[1], rgb[0]);
    let mut line = |k: &str, v: &dyn std::fmt::Display| {
        let _ = write!(s, "{k}: {v}\r\n");
    };
    line("Re", &view.re_string());
    line("Im", &view.im_string());
    line("Zoom", &format_args!("{:.16}", view.zoom));
    line("Iterations", &view.maxiter);
    line("IterDiv", &format_args!("{:.6}", c.iter_div));
    line("ColorMethod", &c.transfer.kf2_method());
    line("Differences", &0);
    line("ColorOffset", &(c.offset.rem_euclid(1024.0) as u32));
    line("Colors", &c.palette.keys().iter().map(bgr).collect::<String>());
    line("InteriorColor", &bgr(&c.interior));
    line("Smooth", &(c.smooth as u8));
    line("Flat", &(c.flat as u8));
    line("Power", &2);
    line("FractalType", &0);
    let sl = c.slopes.unwrap_or_default();
    line("Slopes", &(c.slopes.is_some() as u8));
    line("SlopePower", &sl.power);
    line("SlopeRatio", &sl.ratio);
    line("SlopeAngle", &sl.angle_deg);
    line("ImagPointsUp", &(!view.imag_down as u8));
    s
}

pub fn load(path: &Path) -> Result<Location, String> {
    let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let text = if is_png {
        crate::image::read_png_comment(path)?.ok_or("PNG has no embedded location")?
    } else {
        std::fs::read_to_string(path).map_err(|e| e.to_string())?
    };
    parse(&text)
}

pub fn save(path: &Path, view: &View, c: &Colouring) -> Result<(), String> {
    std::fs::write(path, to_string(view, c)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let text = "Re: -1.7400623825793399052208441670658256382966417204361718668798624184611829\r\n\
                    Im: 0.0281753397792110489924115211443195096875390767429906085704013095958801\r\n\
                    Zoom: 2.0000000000000000E66\r\nIterations: 12345\r\nIterDiv: 0.010000\r\n\
                    Colors: 255,255,255,128,0,64,\r\nSlopes: 1\r\nSlopePower: 50\r\n";
        let loc = parse(text).unwrap();
        assert_eq!(loc.view.maxiter, 12345);
        assert!(loc.view.imag_down);
        assert_eq!(loc.colouring.palette.keys(), &[[255, 255, 255], [64, 0, 128]]);
        assert_eq!(format!("{:.4}", loc.view.zoom), "2.0000E66");
        let again = parse(&to_string(&loc.view, &loc.colouring)).unwrap();
        assert_eq!(again.view.re_string(), loc.view.re_string());
        assert_eq!(again.view.im_string(), loc.view.im_string());
        assert_eq!(again.colouring, loc.colouring);
        assert!(
            loc.view.re_string().starts_with("-1.74006238257933990522084416706582563829664172043617186687986241846118")
        );
    }
}
