//! CUDA pixel engine for fracst.
//!
//! [`CudaEngine`] implements [`fracst_core::PixelEngine`]: the CPU still
//! computes the reference orbit and BLA tables, and the GPU iterates every
//! pixel of a pass. The kernel (`kernel/fracst.cuh`) is a direct port of the
//! CPU pixel loop and produces bit-identical frames.
//!
//! Nothing CUDA is needed to build this crate. At run time it loads the
//! CUDA driver and NVRTC (the runtime compiler) and compiles the kernel for
//! the GPU it finds; [`CudaEngine::new`] fails cleanly if either is missing.
//!
//! With the `host-kernel` feature the same kernel source is also compiled
//! for the CPU ([`host::HostEngine`]), so the port can be tested anywhere.

pub mod check;
pub mod cuda;
#[cfg(feature = "host-kernel")]
pub mod host;
pub mod layout;

pub use cuda::CudaEngine;
