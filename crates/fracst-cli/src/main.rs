use clap::Parser;
use fracst_core::{Colouring, FloatExp, Progress, Renderer, Settings, View, image, kfr, newton};
use std::path::PathBuf;
use std::time::Instant;

/// Render a Mandelbrot set location to PNG.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Location file: .kfr, or a PNG with an embedded location.
    location: Option<PathBuf>,
    /// Real part of the centre (decimal, any number of digits).
    #[arg(long, allow_hyphen_values = true)]
    re: Option<String>,
    /// Imaginary part of the centre.
    #[arg(long, allow_hyphen_values = true)]
    im: Option<String>,
    /// Zoom (the image height spans 4 / zoom), e.g. 1e100.
    #[arg(long)]
    zoom: Option<String>,
    /// Maximum iterations.
    #[arg(short, long)]
    iterations: Option<u64>,
    #[arg(short = 'W', long, default_value_t = 1280)]
    width: u32,
    #[arg(short = 'H', long, default_value_t = 720)]
    height: u32,
    /// Render at N x N samples per pixel and downscale.
    #[arg(long, default_value_t = 1)]
    supersample: u32,
    #[arg(short, long, default_value = "fracst.png")]
    output: PathBuf,
    /// Disable bilinear approximation (plain perturbation).
    #[arg(long)]
    no_bla: bool,
    /// Choose the iteration limit automatically (the file's or --iterations
    /// value is the starting point): enough for minibrots near the view,
    /// raised when pixels are cut off.
    #[arg(long)]
    auto_iterations: bool,
    /// Keep the reference at the view centre instead of snapping it to a
    /// nearby minibrot nucleus.
    #[arg(long)]
    no_snap: bool,
    /// Disable interior detection (iterate interior pixels to the limit).
    #[arg(long)]
    no_interior: bool,
    /// Compute distance estimates (stored in the frame; slower).
    #[arg(long)]
    de: bool,
    /// BLA tolerance as a power of two (epsilon = 2^-bits).
    #[arg(long, default_value_t = 40)]
    bla_bits: u32,
    /// Iterations per palette step (IterDiv); overrides the file.
    #[arg(long)]
    iter_div: Option<f64>,
    /// Disable slope shading.
    #[arg(long)]
    no_slopes: bool,
    /// Render N times and report the fastest (reference reused after the first).
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// Before rendering, jump to the lowest-period minibrot in view.
    #[arg(long)]
    find_minibrot: bool,
    /// With --find-minibrot: zoom relative to the minibrot (1 = it fills the image height).
    #[arg(long, default_value_t = 1.0)]
    mini_zoom: f64,
    /// Render a zoom-out sequence of N frames (for zoom videos):
    /// frames are written as <output stem>-00000.png, -00001.png, ...
    #[arg(long, value_name = "N")]
    zoom_out: Option<u32>,
    /// Zoom factor between frames of a zoom-out sequence.
    #[arg(long, default_value_t = 2.0)]
    zoom_out_factor: f64,
    /// Also write the location as a .kfr file next to the PNG.
    #[arg(long)]
    save_kfr: bool,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let (mut view, mut colouring) = match &args.location {
        Some(path) => {
            let loc = kfr::load(path)?;
            (loc.view, loc.colouring)
        }
        None => (View::default(), Colouring::default()),
    };
    if args.re.is_some() || args.im.is_some() || args.zoom.is_some() {
        let re = args.re.clone().unwrap_or_else(|| view.re_string());
        let im = args.im.clone().unwrap_or_else(|| view.im_string());
        let zoom = args.zoom.clone().unwrap_or_else(|| format!("{:.16}", view.zoom));
        let imag_down = view.imag_down;
        view = View::parse(&re, &im, &zoom, view.maxiter).ok_or("invalid --re/--im/--zoom")?;
        view.imag_down = imag_down;
    }
    if let Some(n) = args.iterations {
        view.maxiter = n;
    }
    if let Some(d) = args.iter_div {
        colouring.iter_div = d;
    }
    if args.no_slopes {
        colouring.slopes = None;
    }
    if args.find_minibrot {
        let t = Instant::now();
        let radius = view.pixel_spacing(args.height) * FloatExp::from(args.height as f64 / 2.0);
        // The probe orbit stops when it escapes, so a generous period bound is cheap.
        let maxperiod = if args.auto_iterations { view.maxiter.max(1 << 24) } else { view.maxiter };
        let nucleus = newton::find_minibrot(&view.re, &view.im, radius, maxperiod, &Progress::default())
            .ok_or("no minibrot found in view")?;
        println!("minibrot: period {} size {:.3} ({:.3}s)", nucleus.period, nucleus.size, t.elapsed().as_secs_f64());
        view.re = nucleus.re;
        view.im = nucleus.im;
        view.zoom = FloatExp::from(args.mini_zoom) / nucleus.size;
    }
    let ss = args.supersample.max(1);
    let settings = Settings {
        width: args.width * ss,
        height: args.height * ss,
        bla: !args.no_bla,
        bla_epsilon: 0.5f64.powi(args.bla_bits as i32),
        interior_detection: !args.no_interior,
        auto_iterations: args.auto_iterations,
        periodic_reference: !args.no_snap,
        distance_estimate: args.de,
        ..Settings::default()
    };
    view.ensure_precision(settings.height);

    if let Some(frames) = args.zoom_out {
        return zoom_out(&args, view, &colouring, &settings, frames);
    }
    println!("centre  {} {}", short(&view.re_string()), short(&view.im_string()));
    println!("zoom    {:.6}  iterations {}  size {}x{}", view.zoom, view.maxiter, settings.width, settings.height);
    let mut renderer = Renderer::new();
    let mut best = None;
    for i in 0..args.repeat.max(1) {
        let t = Instant::now();
        let frame = renderer.render(&view, &settings, &Progress::default()).ok_or("cancelled")?;
        let elapsed = t.elapsed();
        let st = &frame.stats;
        println!(
            "run {}: {:.3}s total | reference {:.3}s ({} iters{}{}, {} limbs) | bla {:.3}s ({} levels) | pixels {:.3}s [{}]",
            i + 1,
            elapsed.as_secs_f64(),
            st.reference.as_secs_f64(),
            st.reference_len,
            if st.reference_escaped {
                ", escaped"
            } else if st.reference_truncated {
                ", truncated"
            } else {
                ""
            },
            if st.reference_reused { ", reused" } else { "" },
            st.reference_limbs,
            st.bla.as_secs_f64(),
            st.bla_levels,
            st.pixels.as_secs_f64(),
            st.mode,
        );
        if best.as_ref().is_none_or(|(d, _)| elapsed < *d) {
            best = Some((elapsed, frame));
        }
    }
    let (_, frame) = best.unwrap();
    let st = &frame.stats;
    let pixels = (frame.width * frame.height) as f64;
    println!(
        "pixels: avg {:.0} iters, {:.1} steps ({:.1} extended range, {:.0}x skipped by BLA) | escaped {}..{} | interior {:.1}%",
        st.iterations as f64 / pixels,
        st.steps as f64 / pixels,
        st.deep_steps as f64 / pixels,
        st.iterations as f64 / st.steps.max(1) as f64,
        st.min_escaped,
        st.max_escaped,
        100.0 * st.interior as f64 / pixels,
    );
    println!(
        "iterations: limit {} ({} pass{}){}{}",
        frame.maxiter,
        st.rounds,
        if st.rounds == 1 { "" } else { "es" },
        st.nearby_period.map(|p| format!(", minibrot of period {p} nearby")).unwrap_or_default(),
        if settings.auto_iterations {
            format!(", next frame would start at {}", st.suggested_maxiter)
        } else {
            String::new()
        },
    );
    if settings.auto_iterations {
        view.maxiter = frame.maxiter;
    }

    let rgba = image::downscale(&colouring.apply(&frame), frame.width, frame.height, ss);
    let comment = kfr::to_string(&view, &colouring);
    image::save_png(&args.output, &rgba, args.width, args.height, Some(&comment))?;
    println!("wrote {}", args.output.display());
    if args.save_kfr {
        let path = args.output.with_extension("kfr");
        kfr::save(&path, &view, &colouring)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

/// Render frames from the given zoom outwards, reusing the reference orbit.
fn zoom_out(
    args: &Args,
    mut view: View,
    colouring: &Colouring,
    settings: &Settings,
    frames: u32,
) -> Result<(), String> {
    let ss = args.supersample.max(1);
    let stem = args.output.with_extension("");
    let mut renderer = Renderer::new();
    let started = Instant::now();
    for i in 0..frames {
        if view.zoom < FloatExp::from(0.5) {
            println!("reached zoom < 0.5 after {i} frames");
            break;
        }
        let t = Instant::now();
        let frame = renderer.render(&view, settings, &Progress::default()).ok_or("cancelled")?;
        let rgba = image::downscale(&colouring.apply(&frame), frame.width, frame.height, ss);
        let path = PathBuf::from(format!("{}-{i:05}.png", stem.display()));
        image::save_png(&path, &rgba, args.width, args.height, Some(&kfr::to_string(&view, colouring)))?;
        println!(
            "frame {i:5}  zoom {:.4}  {:.3}s{}  -> {}",
            view.zoom,
            t.elapsed().as_secs_f64(),
            if frame.stats.reference_reused { " (reference reused)" } else { "" },
            path.display()
        );
        view.zoom = view.zoom / FloatExp::from(args.zoom_out_factor);
        if settings.auto_iterations {
            view.maxiter = frame.stats.suggested_maxiter;
        }
    }
    println!("total {:.2}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn short(s: &str) -> String {
    if s.len() > 40 { format!("{}…({} digits)", &s[..40], s.len()) } else { s.to_string() }
}
