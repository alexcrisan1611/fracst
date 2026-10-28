//! Lossless frame compression for the wire.
//!
//! A frame's arrays (iteration counts, then the bits of the fractional parts
//! and distance estimates) are stored as differences from the left
//! neighbour, laid out as byte planes: all first bytes, then all second
//! bytes, and so on. Neighbouring pixels have similar values, so the high
//! planes compress well; the low bits of detailed regions are close to
//! noise, which puts a detailed 1080p frame at roughly 6-9 MB. The result is cut into blocks of
//! [`BLOCK`] bytes, each compressed separately with zstd, so both ends work
//! on blocks in parallel and each block fits in one modest WebSocket message.

use fracst_core::Frame;
use rayon::prelude::*;

/// Uncompressed bytes per block.
pub const BLOCK: usize = 1 << 20;
const LEVEL: i32 = 3;

/// Spread `words` into four byte planes in `out` (`4 * words.len()` bytes).
fn shuffle(words: impl IndexedParallelIterator<Item = u32>, out: &mut [u8]) {
    let len = out.len() / 4;
    let (p0, rest) = out.split_at_mut(len);
    let (p1, rest) = rest.split_at_mut(len);
    let (p2, p3) = rest.split_at_mut(len);
    words.zip(p0).zip(p1).zip(p2).zip(p3).for_each(|((((w, a), b), c), d)| {
        let [x0, x1, x2, x3] = w.to_le_bytes();
        (*a, *b, *c, *d) = (x0, x1, x2, x3);
    });
}

fn unshuffle(planes: &[u8]) -> Vec<u32> {
    let len = planes.len() / 4;
    (0..len)
        .into_par_iter()
        .map(|i| u32::from_le_bytes([planes[i], planes[len + i], planes[2 * len + i], planes[3 * len + i]]))
        .collect()
}

/// The frame's compressed blocks.
pub fn encode(frame: &Frame) -> Vec<Vec<u8>> {
    let px = frame.n.len();
    let w = frame.width as usize;
    let planes = if frame.de.is_empty() { 2 } else { 3 };
    let mut raw = vec![0u8; 4 * px * planes];
    let (n_part, rest) = raw.split_at_mut(4 * px);
    let (frac_part, de_part) = rest.split_at_mut(4 * px);
    let delta = |v: &[u32], i: usize| if i.is_multiple_of(w) { v[i] } else { v[i].wrapping_sub(v[i - 1]) };
    shuffle((0..px).into_par_iter().map(|i| delta(&frame.n, i)), n_part);
    let bits = |f: &[f32]| -> Vec<u32> { f.par_iter().map(|x| x.to_bits()).collect() };
    let frac = bits(&frame.frac);
    shuffle((0..px).into_par_iter().map(|i| delta(&frac, i)), frac_part);
    if !frame.de.is_empty() {
        let de = bits(&frame.de);
        shuffle((0..px).into_par_iter().map(|i| delta(&de, i)), de_part);
    }
    raw.par_chunks(BLOCK).map(|b| zstd::bulk::compress(b, LEVEL).expect("zstd compression")).collect()
}

/// Rebuild a frame's arrays from its blocks; `stats` are set by the caller.
pub fn decode<B: AsRef<[u8]> + Sync>(
    width: u32,
    height: u32,
    maxiter: u64,
    de: bool,
    blocks: &[B],
) -> Result<Frame, String> {
    let px = width as usize * height as usize;
    let total = 4 * px * if de { 3 } else { 2 };
    if blocks.len() != total.div_ceil(BLOCK) {
        return Err(format!("expected {} blocks, got {}", total.div_ceil(BLOCK), blocks.len()));
    }
    let parts: Vec<Vec<u8>> = blocks
        .par_iter()
        .enumerate()
        .map(|(k, b)| {
            let size = BLOCK.min(total - k * BLOCK);
            let raw = zstd::bulk::decompress(b.as_ref(), size).map_err(|e| format!("block {k}: {e}"))?;
            if raw.len() == size { Ok(raw) } else { Err(format!("block {k} has {} bytes, not {size}", raw.len())) }
        })
        .collect::<Result<_, String>>()?;
    let raw = parts.concat();
    let w = width as usize;
    // Undo the differences, a row at a time.
    let words = |range: std::ops::Range<usize>| -> Vec<u32> {
        let mut v = unshuffle(&raw[range]);
        v.par_chunks_mut(w).for_each(|row| {
            for x in 1..row.len() {
                row[x] = row[x].wrapping_add(row[x - 1]);
            }
        });
        v
    };
    let floats = |range| words(range).into_iter().map(f32::from_bits).collect();
    Ok(Frame {
        width,
        height,
        maxiter,
        n: words(0..4 * px),
        frac: floats(4 * px..8 * px),
        de: if de { floats(8 * px..12 * px) } else { Vec::new() },
        stats: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let (w, h) = (1500u32, 1000u32);
        let px = (w * h) as usize;
        let frame = Frame {
            width: w,
            height: h,
            maxiter: 5000,
            n: (0..px).map(|i| if i % 97 == 0 { u32::MAX } else { (i as u32 / 7) % 5000 }).collect(),
            frac: (0..px).map(|i| (i as f32 * 0.37).fract()).collect(),
            de: (0..px).map(|i| i as f32 * 1e-3).collect(),
            stats: Default::default(),
        };
        let blocks = encode(&frame);
        assert!(blocks.len() > 1);
        let back = decode(w, h, 5000, true, &blocks).unwrap();
        assert_eq!(back.n, frame.n);
        assert!(back.frac.iter().zip(&frame.frac).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert!(back.de.iter().zip(&frame.de).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert!(decode(w, h, 5000, false, &blocks).is_err());
    }
}
