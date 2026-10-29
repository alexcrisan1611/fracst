//! Keyframe files: one frame's raw iteration data, compressed losslessly
//! with the remote protocol's codec, so that colours are applied when
//! encoding and can change without rendering again.
//!
//! Layout (little-endian): `FRACSTKF`, version `u32`, width and height
//! `u32`, the iteration limit used and the one suggested for the next
//! frame `u64`, a distance-estimate flag `u8`, the zoom as mantissa `f64`
//! and exponent `i32`, the block count `u32`, then each block as a `u32`
//! length and its bytes.

use fracst_core::{FloatExp, Frame};
use fracst_remote::codec;
use std::io::{Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"FRACSTKF";
const VERSION: u32 = 1;

pub struct Keyframe {
    pub frame: Frame,
    pub zoom: FloatExp,
    /// Iteration limit the next keyframe out can start from.
    pub suggested_maxiter: u64,
}

/// Write atomically: a crash leaves either the whole file or none.
pub fn save(path: &Path, frame: &Frame, zoom: FloatExp) -> Result<(), String> {
    let blocks = codec::encode(frame);
    let mut out = Vec::with_capacity(64 + blocks.iter().map(|b| b.len() + 4).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&frame.maxiter.to_le_bytes());
    out.extend_from_slice(&frame.stats.suggested_maxiter.to_le_bytes());
    out.push(!frame.de.is_empty() as u8);
    let (m, e) = zoom.parts();
    out.extend_from_slice(&m.to_le_bytes());
    out.extend_from_slice(&e.to_le_bytes());
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for b in &blocks {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    let tmp = path.with_extension("tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Keyframe, String> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut data))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&data).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(data: &[u8]) -> Result<Keyframe, String> {
    let mut rest = data;
    let mut take = move |n: usize| -> Result<&'_ [u8], String> {
        if rest.len() < n {
            return Err("truncated keyframe".into());
        }
        let (s, r) = rest.split_at(n);
        rest = r;
        Ok(s)
    };
    if take(8)? != MAGIC {
        return Err("not a fracst keyframe".into());
    }
    let u32_ = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let u64_ = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    let version = u32_(take(4)?);
    if version != VERSION {
        return Err(format!("keyframe version {version} is not supported"));
    }
    let width = u32_(take(4)?);
    let height = u32_(take(4)?);
    let maxiter = u64_(take(8)?);
    let suggested_maxiter = u64_(take(8)?);
    let de = take(1)?[0] != 0;
    let m = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let e = i32::from_le_bytes(take(4)?.try_into().unwrap());
    let count = u32_(take(4)?) as usize;
    let mut blocks = Vec::with_capacity(count);
    for _ in 0..count {
        let len = u32_(take(4)?) as usize;
        blocks.push(take(len)?);
    }
    let frame = codec::decode(width, height, maxiter, de, &blocks)?;
    Ok(Keyframe { frame, zoom: FloatExp::new(m, e), suggested_maxiter })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let (w, h) = (40u32, 30u32);
        let px = (w * h) as usize;
        let mut frame = Frame {
            width: w,
            height: h,
            maxiter: 777,
            n: (0..px as u32).map(|i| i % 50).collect(),
            frac: (0..px).map(|i| (i as f32 * 0.1).fract()).collect(),
            de: Vec::new(),
            stats: Default::default(),
        };
        frame.stats.suggested_maxiter = 999;
        let dir = std::env::temp_dir().join(format!("fracst-kf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kf.fkf");
        save(&path, &frame, FloatExp::exp2(-3000.5)).unwrap();
        let kf = load(&path).unwrap();
        assert_eq!(kf.frame.n, frame.n);
        assert_eq!(kf.frame.frac, frame.frac);
        assert_eq!(kf.frame.maxiter, 777);
        assert_eq!(kf.suggested_maxiter, 999);
        assert_eq!(kf.zoom, FloatExp::exp2(-3000.5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
