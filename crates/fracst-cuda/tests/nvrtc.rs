//! The kernels must compile with NVRTC for the GPUs fracst targets. NVRTC
//! needs no GPU, only its library: set `FRACST_NVRTC_TEST=1` with NVRTC on
//! the library path (`PATH` on Windows, `LD_LIBRARY_PATH` on Linux).

use fracst_cuda::cuda::nvrtc_compile;
use fracst_cuda::layout::{KERNELS, kernel_source};

#[test]
fn kernels_compile_with_nvrtc() {
    if std::env::var_os("FRACST_NVRTC_TEST").is_none() {
        eprintln!("skipped: set FRACST_NVRTC_TEST=1 with NVRTC available");
        return;
    }
    let src = kernel_source();
    // A100, H100/H200, B200/GB200, B300/GB300, RTX 50.
    for arch in ["sm_80", "sm_90", "sm_100", "sm_103", "sm_120"] {
        let option = format!("--gpu-architecture={arch}");
        match nvrtc_compile(&src, &[&option, "--ptxas-options=-v"], true) {
            Ok((cubin, log)) => {
                eprintln!("{arch}: {} byte CUBIN", cubin.len());
                // Registers and spills per kernel, from ptxas.
                for line in log.lines().filter(|l| l.contains("Used") || l.contains("spill")) {
                    eprintln!("  {}", line.trim());
                }
            }
            Err(log) => panic!("{arch} failed:\n{log}"),
        }
    }
    let (ptx, _) = nvrtc_compile(&src, &["--gpu-architecture=compute_80"], false).expect("PTX");
    let ptx = String::from_utf8_lossy(&ptx);
    for kernel in KERNELS {
        assert!(ptx.contains(&format!(".entry {kernel}")), "PTX lacks {kernel}");
    }
}
