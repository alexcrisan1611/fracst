//! fracst-server: renders frames for remote fracst clients, on a CUDA GPU
//! when one is available (see the library docs).

use clap::Parser;
use fracst_core::PixelEngine;
use fracst_cuda::{CudaEngine, check};
use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Address to listen on. Use 0.0.0.0:7878 to accept connections from other machines.
    #[arg(long, default_value = "127.0.0.1:7878")]
    listen: String,
    /// Require `Authorization: Bearer TOKEN` from clients (default: $FRACST_TOKEN).
    #[arg(long)]
    token: Option<String>,
    /// Render on the CPU even when a GPU is available.
    #[arg(long)]
    cpu: bool,
    /// CUDA device number.
    #[arg(long, default_value_t = 0)]
    device: usize,
    /// Largest image a client may request, in pixels.
    #[arg(long, default_value_t = 1 << 26)]
    max_pixels: u64,
    /// Close connections that send no requests for this many seconds (0: never).
    #[arg(long, default_value_t = 0)]
    idle_timeout: u64,
    /// Check that GPU frames match CPU frames exactly on test views, then exit.
    #[arg(long)]
    selftest: bool,
    /// Time the CPU and the GPU on the benchmark locations, then exit.
    #[arg(long)]
    bench: bool,
    /// With --selftest or --bench: print a JSON report.
    #[arg(long)]
    json: bool,
    /// Compile the kernels with NVRTC for these architectures (no GPU
    /// needed), to check an installation, then exit. E.g. sm_90,sm_100.
    #[arg(long, value_name = "ARCHS", value_delimiter = ',')]
    nvrtc_check: Vec<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if !args.nvrtc_check.is_empty() {
        return nvrtc_check(&args.nvrtc_check);
    }
    let engine: Option<Arc<dyn PixelEngine>> = if args.cpu {
        None
    } else {
        match CudaEngine::new(args.device) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                eprintln!("fracst-server: no CUDA engine ({}); rendering on the CPU", e.lines().next().unwrap_or(""));
                None
            }
        }
    };
    let engine_name = match &engine {
        Some(e) => e.name(),
        None => format!("CPU: {} threads", std::thread::available_parallelism().map_or(1, |n| n.get())),
    };
    eprintln!("fracst-server {}: {engine_name}", env!("CARGO_PKG_VERSION"));
    if args.selftest {
        return selftest(engine, &engine_name, args.json);
    }
    if args.bench {
        bench(engine, &engine_name, args.json);
        return ExitCode::SUCCESS;
    }
    let token = args.token.or_else(|| std::env::var("FRACST_TOKEN").ok()).filter(|t| !t.is_empty());
    let listener = match TcpListener::bind(&args.listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fracst-server: cannot listen on {}: {e}", args.listen);
            return ExitCode::FAILURE;
        }
    };
    eprintln!("listening on ws://{}", listener.local_addr().map_or(args.listen.clone(), |a| a.to_string()));
    fracst_server::run(
        listener,
        fracst_server::Config {
            engine,
            engine_name,
            token,
            max_pixels: args.max_pixels,
            idle_timeout: (args.idle_timeout > 0).then(|| Duration::from_secs(args.idle_timeout)),
        },
    );
    ExitCode::SUCCESS
}

fn selftest(engine: Option<Arc<dyn PixelEngine>>, engine_name: &str, json: bool) -> ExitCode {
    let Some(engine) = engine else {
        eprintln!("selftest: no GPU engine to test");
        return ExitCode::FAILURE;
    };
    let outcomes: Vec<check::Outcome> = check::cases().iter().map(|case| check::run(case, &engine)).collect();
    let passed = outcomes.iter().all(check::Outcome::identical);
    if json {
        let cases: Vec<_> = outcomes
            .iter()
            .map(|o| {
                serde_json::json!({
                    "name": o.name,
                    "identical": o.identical(),
                    "differing_pixels": o.differing,
                    "pixels": o.pixels,
                    "steps_equal": o.steps_equal,
                    "engine_passes": o.engine_passes,
                    "cpu_s": o.cpu.as_secs_f64(),
                    "engine_s": o.engine.as_secs_f64(),
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "engine": engine_name, "passed": passed, "cases": cases }));
    } else {
        for o in &outcomes {
            let verdict = if o.identical() { "identical" } else { "DIFFERENT" };
            println!(
                "{:52} {verdict:9} {:?} of {} pixels differ, {} engine passes, cpu {:.3}s, gpu {:.3}s",
                o.name,
                o.differing,
                o.pixels,
                o.engine_passes,
                o.cpu.as_secs_f64(),
                o.engine.as_secs_f64()
            );
        }
        println!("{}", if passed { "selftest passed" } else { "selftest FAILED" });
    }
    if passed { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn bench(engine: Option<Arc<dyn PixelEngine>>, engine_name: &str, json: bool) {
    let mut rows = Vec::new();
    if !json {
        println!(
            "{:34} {:>10} {:>10} {:>10} {:>10} {:>8}",
            "location", "cpu first", "cpu again", "gpu first", "gpu again", "speedup"
        );
    }
    for case in check::bench_cases() {
        let cpu = check::time(&case, None, 2);
        let gpu = engine.as_ref().map(|e| check::time(&case, Some(e.clone()), 3));
        let speedup = gpu.as_ref().map(|g| cpu.again.as_secs_f64() / g.again.as_secs_f64());
        if json {
            rows.push(serde_json::json!({
                "name": case.name,
                "cpu_first_s": cpu.first.as_secs_f64(),
                "cpu_again_s": cpu.again.as_secs_f64(),
                "cpu_pixels_s": cpu.pixels.as_secs_f64(),
                "gpu_first_s": gpu.as_ref().map(|g| g.first.as_secs_f64()),
                "gpu_again_s": gpu.as_ref().map(|g| g.again.as_secs_f64()),
                "gpu_pixels_s": gpu.as_ref().map(|g| g.pixels.as_secs_f64()),
                "speedup": speedup,
            }));
        } else {
            let s = |d: Option<Duration>| d.map_or("-".into(), |d| format!("{:.3}s", d.as_secs_f64()));
            println!(
                "{:34} {:>10} {:>10} {:>10} {:>10} {:>8}",
                case.name,
                s(Some(cpu.first)),
                s(Some(cpu.again)),
                s(gpu.as_ref().map(|g| g.first)),
                s(gpu.as_ref().map(|g| g.again)),
                speedup.map_or("-".into(), |x| format!("{x:.1}x"))
            );
        }
    }
    if json {
        println!("{}", serde_json::json!({ "engine": engine_name, "width": 1920, "height": 1080, "locations": rows }));
    }
}

fn nvrtc_check(archs: &[String]) -> ExitCode {
    let src = fracst_cuda::layout::kernel_source();
    let mut ok = true;
    for arch in archs {
        let option = format!("--gpu-architecture={arch}");
        // cudarc panics if the NVRTC library is missing.
        let compiled = std::panic::catch_unwind(|| fracst_cuda::cuda::nvrtc_compile(&src, &[&option], true));
        match compiled {
            Ok(Ok((cubin, _))) => println!("{arch}: compiled, {} byte CUBIN", cubin.len()),
            Ok(Err(log)) => {
                ok = false;
                println!(
                    "{arch}: failed
{log}"
                );
            }
            Err(_) => {
                println!("NVRTC library not found");
                return ExitCode::FAILURE;
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
