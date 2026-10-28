//! The kernel, compiled for the host, must reproduce the CPU loop's frames
//! exactly. Run with `cargo test -p fracst-cuda --features host-kernel`.

#![cfg(feature = "host-kernel")]

use fracst_core::PixelEngine;
use fracst_cuda::host::{HostEngine, kernel_struct_sizes};
use fracst_cuda::layout::{BlaX, Params, PixelOut};
use std::sync::Arc;

#[test]
fn layouts_match_the_kernel() {
    assert_eq!(kernel_struct_sizes(), [size_of::<Params>(), size_of::<BlaX>(), size_of::<PixelOut>()]);
}

#[test]
fn host_kernel_matches_cpu() {
    let engine: Arc<dyn PixelEngine> = Arc::new(HostEngine);
    let mut failures = Vec::new();
    for case in fracst_cuda::check::cases() {
        let o = fracst_cuda::check::run(&case, &engine);
        eprintln!(
            "{}: {:?} of {} pixels differ, steps equal: {}, engine passes: {} (cpu {:.0?}, host kernel {:.0?})",
            o.name, o.differing, o.pixels, o.steps_equal, o.engine_passes, o.cpu, o.engine
        );
        if !o.identical() {
            failures.push(o.name);
        }
    }
    assert!(failures.is_empty(), "frames differ: {failures:?}");
}
