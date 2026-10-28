//! The kernel's pixel loop compiled for the host CPU (feature `host-kernel`).
//! It runs the exact code the GPU runs, so it checks the port against the
//! CPU loop on machines without an NVIDIA GPU.

use crate::layout::{self, Params, PixelOut, Pointers, Tables};
use fracst_core::perturb::PixelResult;
use fracst_core::{EnginePass, PixelEngine};
use rayon::prelude::*;

unsafe extern "C" {
    fn fracst_host_iterate(p: *const Params, deep: i32, de: i32, inside: i32, begin: u64, end: u64);
    fn fracst_host_params_size() -> u64;
    fn fracst_host_bla_x_size() -> u64;
    fn fracst_host_pixel_out_size() -> u64;
}

/// Sizes of `Params`, `BlaX` and `PixelOut` as the C++ compiler laid them out.
pub fn kernel_struct_sizes() -> [usize; 3] {
    // SAFETY: the functions only return `sizeof` values.
    unsafe { [fracst_host_params_size(), fracst_host_bla_x_size(), fracst_host_pixel_out_size()].map(|s| s as usize) }
}

/// Runs the kernel's pixel loop on the CPU, in parallel.
pub struct HostEngine;

impl PixelEngine for HostEngine {
    fn name(&self) -> String {
        "host build of the CUDA kernel".into()
    }

    fn iterate(&self, pass: &EnginePass<'_>) -> Option<Vec<PixelResult>> {
        const CHUNK: usize = 64;
        let count = pass.dcs.len();
        let tables = Tables::new(pass);
        let deep_dcs = layout::deep_dcs(pass);
        let mut outs = vec![PixelOut::default(); count];
        let at = |v: *const u8| v as u64;
        let params = layout::params(
            pass,
            Pointers {
                orbit: at(layout::orbit(pass).as_ptr().cast()),
                deep_orbit: at(tables.deep_orbit.as_ptr().cast()),
                bla64: at(tables.bla64.entries.as_ptr().cast()),
                levels64: at(tables.bla64.levels.as_ptr().cast()),
                bladeep: at(tables.bladeep.entries.as_ptr().cast()),
                levelsdeep: at(tables.bladeep.levels.as_ptr().cast()),
                dcs: at(pass.dcs.as_ptr().cast()),
                deep_dcs: at(deep_dcs.as_ptr().cast()),
                out: at(outs.as_mut_ptr().cast()),
                counter: 0,
                cancel: 0,
            },
        );
        let (deep, de, inside) =
            (pass.deep.is_some() as i32, pass.extras.distance_estimate as i32, pass.extras.interior_detection as i32);
        (0..count.div_ceil(CHUNK)).into_par_iter().for_each(|chunk| {
            if pass.progress.cancelled() {
                return;
            }
            let (begin, end) = (chunk * CHUNK, ((chunk + 1) * CHUNK).min(count));
            // SAFETY: `params` points at buffers that outlive this loop, and
            // each chunk writes only its own range of `outs`.
            unsafe { fracst_host_iterate(&params, deep, de, inside, begin as u64, end as u64) };
        });
        if pass.progress.cancelled() {
            return None;
        }
        Some(layout::results(&outs))
    }
}
