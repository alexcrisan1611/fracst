//! Data shared with the kernel. Every `#[repr(C)]` type here mirrors a
//! struct of the same name in `kernel/fracst.cuh`.

use fracst_core::bla::BlaTable;
use fracst_core::num::{CExp, Complex};
use fracst_core::perturb::{INTERIOR_RADIUS2, INTERIOR_START, PixelResult};
use fracst_core::{EnginePass, FloatExp};
use rayon::prelude::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FExp {
    pub m: f64,
    pub e: i32,
    pub pad: i32,
}

impl From<FloatExp> for FExp {
    fn from(x: FloatExp) -> Self {
        let (m, e) = x.parts();
        FExp { m, e, pad: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CX {
    pub re: f64,
    pub im: f64,
    pub e: i32,
    pub pad: i32,
}

impl From<CExp> for CX {
    fn from(c: CExp) -> Self {
        let (re, im, e) = c.parts();
        CX { re, im, e, pad: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DeepZ {
    pub re: f64,
    pub im: f64,
    pub norm: f64,
    pub e: i32,
    pub pad: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BlaF {
    pub ar: f64,
    pub ai: f64,
    pub br: f64,
    pub bi: f64,
    pub r2: f64,
    pub l: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BlaX {
    pub ar: f64,
    pub ai: f64,
    pub br: f64,
    pub bi: f64,
    pub r2m: f64,
    pub ae: i32,
    pub be: i32,
    pub r2e: i32,
    pub l: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PixelOut {
    pub zr: f64,
    pub zi: f64,
    pub dr: f64,
    pub di: f64,
    pub n: u64,
    pub steps: u64,
    pub deep_steps: u64,
    pub escaped: u32,
    pub pad: u32,
}

/// Kernel parameters. The pointer fields hold device addresses on the GPU
/// and host addresses in the host build.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    pub orbit: u64,
    pub deep_orbit: u64,
    pub bla64: u64,
    pub levels64: u64,
    pub bladeep: u64,
    pub levelsdeep: u64,
    pub dcs: u64,
    pub deep_dcs: u64,
    pub out: u64,
    pub counter: u64,
    pub cancel: u64,
    pub count: u64,
    pub mref: u64,
    pub maxiter: u64,
    pub interior_start: u64,
    pub nlevels64: u64,
    pub nlevelsdeep: u64,
    pub periodic: u64,
    pub max_r2_64: f64,
    pub spacing64: f64,
    pub bailout2_64: f64,
    pub switch2_64: f64,
    pub interior_radius2: f64,
    pub max_r2_deep: FExp,
    pub spacing_deep: FExp,
    pub bailout2_deep: FExp,
    pub switch2_deep: FExp,
}

/// Where the kernel finds its inputs and outputs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointers {
    pub orbit: u64,
    pub deep_orbit: u64,
    pub bla64: u64,
    pub levels64: u64,
    pub bladeep: u64,
    pub levelsdeep: u64,
    pub dcs: u64,
    pub deep_dcs: u64,
    pub out: u64,
    pub counter: u64,
    pub cancel: u64,
}

/// A table's steps, all levels one after another, with `(start, len)` per level.
pub struct Levels<T> {
    pub entries: Vec<T>,
    pub levels: Vec<u64>,
}

fn flatten<C: fracst_core::num::Cplx, T: Send>(
    table: &BlaTable<C>,
    convert: impl Fn(&fracst_core::bla::Bla<C>) -> T + Sync,
) -> Levels<T> {
    let mut entries = Vec::new();
    let mut levels = Vec::with_capacity(2 * table.levels());
    for k in 0..table.levels() {
        let level = table.level(k);
        levels.push(entries.len() as u64);
        levels.push(level.len() as u64);
        entries.par_extend(level.par_iter().map(&convert));
    }
    Levels { entries, levels }
}

/// The reference orbit and BLA tables of a pass, laid out for the kernel.
pub struct Tables {
    pub deep_orbit: Vec<DeepZ>,
    pub bla64: Levels<BlaF>,
    pub bladeep: Levels<BlaX>,
}

impl Tables {
    pub fn new(pass: &EnginePass) -> Self {
        let bla64 = flatten(pass.shallow.bla, |b| BlaF {
            ar: b.a.re,
            ai: b.a.im,
            br: b.b.re,
            bi: b.b.im,
            r2: b.r2,
            l: b.l,
            pad: 0,
        });
        let (deep_orbit, bladeep) = match &pass.deep {
            Some(deep) => {
                let orbit = pass
                    .reference
                    .deep_orbit()
                    .par_iter()
                    .map(|d| {
                        let (re, im, e) = d.z.parts();
                        DeepZ { re, im, norm: d.norm, e, pad: 0 }
                    })
                    .collect();
                let table = flatten(deep.bla, |b| {
                    let ((ar, ai, ae), (br, bi, be)) = (b.a.parts(), b.b.parts());
                    let (r2m, r2e) = b.r2.parts();
                    BlaX { ar, ai, br, bi, r2m, ae, be, r2e, l: b.l }
                });
                (orbit, table)
            }
            None => (Vec::new(), Levels { entries: Vec::new(), levels: Vec::new() }),
        };
        Tables { deep_orbit, bla64, bladeep }
    }
}

/// The reference orbit as `(re, im)` pairs.
pub fn orbit<'a>(pass: &EnginePass<'a>) -> &'a [f64] {
    as_f64(pass.reference.orbit())
}

/// Complex numbers as `(re, im)` pairs.
pub fn as_f64(c: &[Complex<f64>]) -> &[f64] {
    // SAFETY: `Complex<f64>` is `repr(C)` with two `f64` fields.
    unsafe { std::slice::from_raw_parts(c.as_ptr().cast::<f64>(), 2 * c.len()) }
}

/// Deep passes: each pixel's offset in extended range.
pub fn deep_dcs(pass: &EnginePass) -> Vec<CX> {
    pass.deep_dcs.par_iter().map(|&c| CX::from(c)).collect()
}

pub fn params(pass: &EnginePass, at: Pointers) -> Params {
    let shallow = &pass.shallow;
    let mut p = Params {
        orbit: at.orbit,
        deep_orbit: at.deep_orbit,
        bla64: at.bla64,
        levels64: at.levels64,
        bladeep: at.bladeep,
        levelsdeep: at.levelsdeep,
        dcs: at.dcs,
        deep_dcs: at.deep_dcs,
        out: at.out,
        counter: at.counter,
        cancel: at.cancel,
        count: pass.dcs.len() as u64,
        mref: pass.reference.last_index() as u64,
        maxiter: pass.maxiter,
        interior_start: INTERIOR_START,
        nlevels64: shallow.bla.levels() as u64,
        nlevelsdeep: 0,
        periodic: pass.reference.truncated.is_some() as u64,
        max_r2_64: shallow.bla.max_r2(),
        spacing64: shallow.spacing,
        bailout2_64: shallow.bailout2,
        switch2_64: shallow.switch2,
        interior_radius2: INTERIOR_RADIUS2,
        ..Params::default()
    };
    if let Some(deep) = &pass.deep {
        p.nlevelsdeep = deep.bla.levels() as u64;
        p.max_r2_deep = deep.bla.max_r2().into();
        p.spacing_deep = deep.spacing.into();
        p.bailout2_deep = deep.bailout2.into();
        p.switch2_deep = deep.switch2.into();
    }
    p
}

/// Kernel outputs as the CPU loop's results.
pub fn results(outs: &[PixelOut]) -> Vec<PixelResult> {
    outs.par_iter()
        .map(|o| PixelResult {
            n: o.n,
            escaped: o.escaped != 0,
            z: Complex::new(o.zr, o.zi),
            der: Complex::new(o.dr, o.di),
            steps: o.steps,
            deep_steps: o.deep_steps,
        })
        .collect()
}

/// Index of the kernel for a pass: `[f64 | deep] x [distance estimate] x [interior detection]`.
pub fn kernel_index(pass: &EnginePass) -> usize {
    (pass.deep.is_some() as usize) << 2
        | (pass.extras.distance_estimate as usize) << 1
        | pass.extras.interior_detection as usize
}

/// Kernel names, in [`kernel_index`] order.
pub const KERNELS: [&str; 8] = [
    "fracst_f64_00",
    "fracst_f64_01",
    "fracst_f64_10",
    "fracst_f64_11",
    "fracst_deep_00",
    "fracst_deep_01",
    "fracst_deep_10",
    "fracst_deep_11",
];

/// The kernel source, for NVRTC.
pub fn kernel_source() -> String {
    format!("{}\n{}", include_str!("../kernel/fracst.cuh"), include_str!("../kernel/kernels.cu"))
}
