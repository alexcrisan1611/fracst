//! The CUDA engine: compiles the kernel with NVRTC for the device it finds,
//! keeps the reference orbit and BLA tables on the device between passes,
//! and iterates every pixel of a pass in one launch.

use crate::layout::{self, BlaF, BlaX, CX, DeepZ, Params, PixelOut, Pointers, Tables};
use cudarc::driver::sys::CUdevice_attribute;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DevicePtr, DeviceRepr, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{Ptx, result as nvrtc, sys as nvrtc_sys};
use fracst_core::perturb::PixelResult;
use fracst_core::{EnginePass, PixelEngine};
use std::ffi::{CStr, CString, c_char};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// SAFETY: plain `repr(C)` data, mirrored by the kernel's structs.
unsafe impl DeviceRepr for Params {}
unsafe impl DeviceRepr for DeepZ {}
unsafe impl DeviceRepr for BlaF {}
unsafe impl DeviceRepr for BlaX {}
unsafe impl DeviceRepr for CX {}
unsafe impl DeviceRepr for PixelOut {}

const BLOCK: u32 = 256;

pub struct CudaEngine {
    name: String,
    inner: Mutex<Inner>,
}

struct Inner {
    ctx: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    /// Writes the cancel flag while a kernel runs on `stream`.
    cancel_stream: Arc<CudaStream>,
    kernels: Vec<CudaFunction>,
    /// Persistent threads: enough blocks to fill every multiprocessor.
    grid: u32,
    counter: CudaSlice<u64>,
    cancel: CudaSlice<i32>,
    tables: Option<DeviceTables>,
}

/// A pass's reference orbit and BLA tables on the device.
struct DeviceTables {
    id: u64,
    orbit: CudaSlice<f64>,
    deep_orbit: CudaSlice<DeepZ>,
    bla64: CudaSlice<BlaF>,
    levels64: CudaSlice<u64>,
    bladeep: CudaSlice<BlaX>,
    levelsdeep: CudaSlice<u64>,
}

impl CudaEngine {
    /// Open CUDA device `ordinal` and compile the kernels for it.
    pub fn new(ordinal: usize) -> Result<Self, String> {
        // cudarc panics when the CUDA driver or NVRTC library is missing.
        let quiet = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(|| Inner::open(ordinal));
        std::panic::set_hook(quiet);
        match result {
            Ok(Ok((name, inner))) => Ok(CudaEngine { name, inner: Mutex::new(inner) }),
            Ok(Err(e)) => Err(e),
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                Err(if message.contains("\"cuda\"") {
                    "no NVIDIA driver found".into()
                } else if message.contains("\"nvrtc\"") {
                    "NVRTC not found: install the CUDA toolkit or the nvidia-cuda-nvrtc package".into()
                } else {
                    format!("CUDA is not available: {message}")
                })
            }
        }
    }
}

impl PixelEngine for CudaEngine {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn iterate(&self, pass: &EnginePass<'_>) -> Option<Vec<PixelResult>> {
        let mut inner = self.inner.lock().ok()?;
        match inner.iterate(pass) {
            Ok(results) => results,
            Err(e) => {
                eprintln!("fracst-cuda: {e}");
                None
            }
        }
    }
}

fn driver<T>(r: Result<T, cudarc::driver::DriverError>) -> Result<T, String> {
    r.map_err(|e| format!("CUDA: {e}"))
}

/// The device address of a buffer. Event tracking is off, so the guard
/// `device_ptr` returns does nothing; all work runs on one stream.
fn addr<T>(s: &CudaSlice<T>, stream: &CudaStream) -> u64 {
    s.device_ptr(stream).0
}

/// Upload `data`, or one default element if it is empty (CUDA cannot allocate zero bytes).
fn upload<T: DeviceRepr + Default + Clone>(stream: &Arc<CudaStream>, data: &[T]) -> Result<CudaSlice<T>, String> {
    if data.is_empty() { driver(stream.clone_htod(&[T::default()])) } else { driver(stream.clone_htod(data)) }
}

impl Inner {
    fn open(ordinal: usize) -> Result<(String, Inner), String> {
        let ctx = driver(CudaContext::new(ordinal))?;
        // SAFETY: everything is ordered on `stream` and synchronised before
        // buffers are reused or dropped; the only other stream writes the
        // cancel flag, which the kernel only reads.
        unsafe { ctx.disable_event_tracking() };
        let (major, minor) = driver(ctx.compute_capability())?;
        let device = driver(ctx.name())?;
        let (image, target) = compile(major, minor)?;
        let module = driver(ctx.load_module(image))?;
        let kernels = layout::KERNELS.iter().map(|k| driver(module.load_function(k))).collect::<Result<Vec<_>, _>>()?;
        let stream = driver(ctx.new_stream())?;
        let cancel_stream = driver(ctx.new_stream())?;
        let sms = driver(ctx.attribute(CUdevice_attribute::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT))? as u32;
        // The heaviest kernel (deep, all extras) bounds how many blocks fit.
        let per_sm = kernels
            .iter()
            .map(|k| k.occupancy_max_active_blocks_per_multiprocessor(BLOCK, 0, None).unwrap_or(1))
            .min()
            .unwrap_or(1)
            .max(1);
        let counter = driver(stream.alloc_zeros::<u64>(1))?;
        let cancel = driver(stream.alloc_zeros::<i32>(1))?;
        let name = format!("CUDA: {device} ({target}, {sms} SMs)");
        let inner = Inner { ctx, stream, cancel_stream, kernels, grid: sms * per_sm, counter, cancel, tables: None };
        Ok((name, inner))
    }

    fn iterate(&mut self, pass: &EnginePass) -> Result<Option<Vec<PixelResult>>, String> {
        driver(self.ctx.bind_to_thread())?;
        let count = pass.dcs.len();
        if count == 0 {
            return Ok(Some(Vec::new()));
        }
        if self.tables.as_ref().is_none_or(|t| t.id != pass.tables_id) {
            self.tables = None;
            let host = Tables::new(pass);
            self.tables = Some(DeviceTables {
                id: pass.tables_id,
                orbit: upload(&self.stream, layout::orbit(pass))?,
                deep_orbit: upload(&self.stream, &host.deep_orbit)?,
                bla64: upload(&self.stream, &host.bla64.entries)?,
                levels64: upload(&self.stream, &host.bla64.levels)?,
                bladeep: upload(&self.stream, &host.bladeep.entries)?,
                levelsdeep: upload(&self.stream, &host.bladeep.levels)?,
            });
        }
        let stream = &self.stream;
        let t = self.tables.as_ref().unwrap();
        let dcs = upload(stream, layout::as_f64(pass.dcs))?;
        let deep_dcs = upload(stream, &layout::deep_dcs(pass))?;
        // SAFETY: the kernel writes every element unless cancelled, and a
        // cancelled pass is discarded unread.
        let out = driver(unsafe { stream.alloc::<PixelOut>(count) })?;
        driver(stream.memset_zeros(&mut self.counter))?;
        driver(stream.memset_zeros(&mut self.cancel))?;
        let params = layout::params(
            pass,
            Pointers {
                orbit: addr(&t.orbit, stream),
                deep_orbit: addr(&t.deep_orbit, stream),
                bla64: addr(&t.bla64, stream),
                levels64: addr(&t.levels64, stream),
                bladeep: addr(&t.bladeep, stream),
                levelsdeep: addr(&t.levelsdeep, stream),
                dcs: addr(&dcs, stream),
                deep_dcs: addr(&deep_dcs, stream),
                out: addr(&out, stream),
                counter: addr(&self.counter, stream),
                cancel: addr(&self.cancel, stream),
            },
        );
        let kernel = &self.kernels[layout::kernel_index(pass)];
        let grid = self.grid.min((count as u32).div_ceil(BLOCK)).max(1);
        let config = LaunchConfig { grid_dim: (grid, 1, 1), block_dim: (BLOCK, 1, 1), shared_mem_bytes: 0 };
        let mut launch = stream.launch_builder(kernel);
        launch.arg(&params);
        // SAFETY: `params` matches the kernel's `Params`, and every buffer it
        // points to outlives the synchronisation below.
        driver(unsafe { launch.launch(config) })?;
        let done = driver(stream.record_event(None))?;
        let mut cancelled = false;
        while !driver(done.try_is_complete())? {
            if !cancelled && pass.progress.cancelled() {
                // Threads stop claiming pixels; the ones in flight finish.
                driver(self.cancel_stream.memcpy_htod(&[1i32], &mut self.cancel))?;
                cancelled = true;
            }
            std::thread::sleep(Duration::from_micros(250));
        }
        driver(stream.synchronize())?;
        if cancelled {
            return Ok(None);
        }
        let outs = driver(stream.clone_dtoh(&out))?;
        Ok(Some(layout::results(&outs)))
    }
}

/// Compile the kernels for compute capability `major.minor`: a CUBIN for
/// that exact architecture (no JIT, so older drivers work), or PTX for a
/// common virtual architecture when NVRTC does not know the device yet.
fn compile(major: i32, minor: i32) -> Result<(Ptx, String), String> {
    let src = layout::kernel_source();
    let arch = format!("sm_{major}{minor}");
    match nvrtc_compile(&src, &[&format!("--gpu-architecture={arch}")], true) {
        Ok((cubin, _)) => Ok((Ptx::from_binary(cubin), arch)),
        Err(first) => {
            let (ptx, _) = nvrtc_compile(&src, &["--gpu-architecture=compute_80"], false)
                .map_err(|e| format!("NVRTC failed for {arch}: {first}\nand for compute_80: {e}"))?;
            let ptx = String::from_utf8_lossy(&ptx).trim_end_matches('\0').to_string();
            Ok((Ptx::from_src(ptx), "compute_80 PTX".into()))
        }
    }
}

/// Run NVRTC on `src` with `extra` options (at least the architecture),
/// returning the CUBIN (`cubin`) or PTX it produced, and the compiler log.
pub fn nvrtc_compile(src: &str, extra: &[&str], cubin: bool) -> Result<(Vec<u8>, String), String> {
    let csrc = CString::new(src).map_err(|e| e.to_string())?;
    let prog = nvrtc::create_program(&csrc, None).map_err(|e| format!("{e:?}"))?;
    // Contraction off: multiply-adds are fused exactly where the CPU loop fuses them.
    let mut options = vec!["--fmad=false", "--std=c++17"];
    options.extend_from_slice(extra);
    // SAFETY: `prog` is a live program; it is destroyed once below.
    unsafe {
        let compiled = nvrtc::compile_program(prog, &options);
        let log = nvrtc::get_program_log(prog)
            .map(|l| CStr::from_ptr(l.as_ptr()).to_string_lossy().into_owned())
            .unwrap_or_else(|e| format!("{e:?}"));
        let output = match compiled {
            Err(_) => Err(log.clone()),
            Ok(()) if cubin => {
                let mut size = 0usize;
                nvrtc_sys::nvrtcGetCUBINSize(prog, &mut size).result().map_err(|e| format!("{e:?}")).and_then(|_| {
                    let mut buf = vec![0u8; size];
                    nvrtc_sys::nvrtcGetCUBIN(prog, buf.as_mut_ptr().cast::<c_char>())
                        .result()
                        .map(|_| buf)
                        .map_err(|e| format!("{e:?}"))
                })
            }
            Ok(()) => nvrtc::get_ptx(prog).map(|p| p.iter().map(|&c| c as u8).collect()).map_err(|e| format!("{e:?}")),
        };
        let _ = nvrtc::destroy_program(prog);
        output.map(|bytes| (bytes, log))
    }
}
