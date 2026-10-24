//! PNG output with the location embedded the way KF2 does it (a `Comment`
//! text chunk holding KFR text), so KF2 and fracst can reopen saved images.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub fn save_png(path: &Path, rgba: &[u8], width: u32, height: u32, comment: Option<&str>) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    write_png(BufWriter::new(file), rgba, width, height, comment)
}

/// Encode an RGBA image as PNG in memory.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    write_png(&mut out, rgba, width, height, None)?;
    Ok(out)
}

fn write_png(
    out: impl std::io::Write,
    rgba: &[u8],
    width: u32,
    height: u32,
    comment: Option<&str>,
) -> Result<(), String> {
    let mut enc = png::Encoder::new(out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    if let Some(c) = comment {
        enc.add_text_chunk("Comment".into(), c.into()).map_err(|e| e.to_string())?;
    }
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())
}

/// Decode an 8-bit RGBA PNG (such as the app icon) from memory.
pub fn decode_rgba_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("expected an 8-bit RGBA PNG".into());
    }
    buf.truncate(info.buffer_size());
    Ok((buf, info.width, info.height))
}

/// The `Comment` text of a PNG, if any.
pub fn read_png_comment(path: &Path) -> Result<Option<String>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_ignore_text_chunk(false);
    let reader = dec.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    let latin = info.uncompressed_latin1_text.iter().map(|t| (&t.keyword, t.text.clone()));
    let utf8 = info.utf8_text.iter().filter_map(|t| Some((&t.keyword, t.get_text().ok()?)));
    let ztxt = info.compressed_latin1_text.iter().filter_map(|t| Some((&t.keyword, t.get_text().ok()?)));
    Ok(latin.chain(utf8).chain(ztxt).find(|(k, _)| k.as_str() == "Comment").map(|(_, t)| t))
}

/// Box-filter an RGBA image down by an integer factor.
pub fn downscale(rgba: &[u8], w: u32, h: u32, k: u32) -> Vec<u8> {
    if k <= 1 {
        return rgba.to_vec();
    }
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..4 {
                let mut sum = 0u32;
                for dy in 0..k {
                    for dx in 0..k {
                        sum += rgba[(((y * k + dy) * w + x * k + dx) * 4 + c) as usize] as u32;
                    }
                }
                out[((y * ow + x) * 4 + c) as usize] = (sum / (k * k)) as u8;
            }
        }
    }
    out
}
