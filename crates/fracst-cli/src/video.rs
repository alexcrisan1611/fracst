//! `fracst-cli video`: zoom videos from the command line.

use fracst_core::kfr;
use fracst_video::{
    Codec, ColourKey, Control, Engine, Event, Length, Outcome, Project, Quality, VideoSpec, Written, ffmpeg,
};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Exit code for a job that paused and can resume.
const PAUSED: i32 = 2;

#[derive(clap::Args)]
pub struct VideoArgs {
    /// Location to zoom into: .kfr, or a PNG with an embedded location.
    /// Not needed to resume a project.
    location: Option<PathBuf>,
    /// Project folder, holding the settings and keyframes. If it holds a
    /// project already, that one resumes (default: next to the location,
    /// named after it with "-video").
    #[arg(short, long)]
    project: Option<PathBuf>,
    /// The video file (default: video.mp4 in the project folder).
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short = 'W', long)]
    width: Option<u32>,
    #[arg(short = 'H', long)]
    height: Option<u32>,
    #[arg(long)]
    fps: Option<f64>,
    /// draft, good or best: keyframe size and samples per pixel.
    #[arg(long, value_parser = parse_quality)]
    quality: Option<Quality>,
    /// Keyframe size relative to the video (overrides the quality's).
    #[arg(long)]
    keyframe_scale: Option<f64>,
    /// Zoom factor between keyframes.
    #[arg(long)]
    keyframe_factor: Option<f64>,
    /// The video starts at this zoom.
    #[arg(long)]
    start_zoom: Option<f64>,
    /// Fixed iteration limit for keyframes instead of automatic ones.
    #[arg(long)]
    no_auto_iterations: bool,
    /// Length of the whole video in seconds.
    #[arg(long, conflicts_with_all = ["per_doubling", "fit_audio"])]
    duration: Option<f64>,
    /// Seconds per doubling of the zoom at full speed.
    #[arg(long, conflicts_with = "fit_audio")]
    per_doubling: Option<f64>,
    /// Make the video exactly as long as the audio track.
    #[arg(long)]
    fit_audio: bool,
    /// Seconds to speed up at the start.
    #[arg(long)]
    ease_in: Option<f64>,
    /// Seconds to slow down before the end.
    #[arg(long)]
    ease_out: Option<f64>,
    /// Seconds to hold still at the start.
    #[arg(long)]
    hold_start: Option<f64>,
    /// Seconds to hold still at the end.
    #[arg(long)]
    hold_end: Option<f64>,
    /// h264, h265, av1 or prores.
    #[arg(long, value_parser = parse_codec)]
    codec: Option<Codec>,
    /// Constant-quality setting for the codec (lower is better).
    #[arg(long)]
    crf: Option<u32>,
    /// Audio track to put under the video.
    #[arg(long)]
    audio: Option<PathBuf>,
    /// Remove the audio track.
    #[arg(long, conflicts_with = "audio")]
    no_audio: bool,
    /// Palette cycling, in palette entries (of 1024) per second.
    #[arg(long, allow_hyphen_values = true)]
    cycle: Option<f64>,
    /// A colour key, TIME:field=value,...; fields are offset, iter_div,
    /// slope_power, slope_ratio and slope_angle. Repeat for more keys; they
    /// replace the project's.
    #[arg(long, value_name = "KEY")]
    colour_key: Vec<String>,
    /// Remove all colour keys.
    #[arg(long, conflicts_with = "colour_key")]
    no_colour_keys: bool,
    /// Render keyframes on a remote fracst server (see the main help).
    #[arg(long, value_name = "URL")]
    remote: Option<String>,
    /// Key for --remote (default: $FRACST_REMOTE_KEY, then $FAL_KEY).
    #[arg(long)]
    key: Option<String>,
    /// Render one keyframe, estimate how long the rest take, and stop.
    #[arg(long)]
    estimate: bool,
    /// Stop after rendering the keyframes.
    #[arg(long)]
    keyframes_only: bool,
    /// Write PNG frames instead of a video file.
    #[arg(long)]
    frames: bool,
    /// ffmpeg to use (default: $FRACST_FFMPEG, a downloaded one, then the PATH).
    #[arg(long)]
    ffmpeg: Option<PathBuf>,
    /// Download ffmpeg without asking if there is none.
    #[arg(long)]
    download_ffmpeg: bool,
}

fn parse_quality(s: &str) -> Result<Quality, String> {
    Quality::parse(s).ok_or_else(|| "expected draft, good or best".into())
}

fn parse_codec(s: &str) -> Result<Codec, String> {
    Codec::parse(s).ok_or_else(|| "expected h264, h265, av1 or prores".into())
}

fn parse_colour_key(s: &str) -> Result<ColourKey, String> {
    let (time, fields) = s.split_once(':').ok_or("a colour key is TIME:field=value,...")?;
    let mut key =
        ColourKey { time: time.trim().parse().map_err(|_| format!("bad time in {s}"))?, ..Default::default() };
    for f in fields.split(',').filter(|f| !f.trim().is_empty()) {
        let (name, value) = f.split_once('=').ok_or_else(|| format!("expected field=value, got {f}"))?;
        let v: f64 = value.trim().parse().map_err(|_| format!("bad value in {f}"))?;
        let slot = match name.trim().replace('-', "_").as_str() {
            "offset" => &mut key.offset,
            "iter_div" => &mut key.iter_div,
            "slope_power" => &mut key.slope_power,
            "slope_ratio" => &mut key.slope_ratio,
            "slope_angle" => &mut key.slope_angle,
            other => return Err(format!("unknown colour key field {other}")),
        };
        *slot = Some(v);
    }
    Ok(key)
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Apply the options that were given to `spec`.
fn apply(a: &VideoArgs, spec: &mut VideoSpec) -> Result<(), String> {
    let mut resize = false;
    if let Some(w) = a.width {
        spec.width = w;
        resize = true;
    }
    if let Some(h) = a.height {
        spec.height = h;
        resize = true;
    }
    if let Some(q) = a.quality {
        spec.quality = q;
        resize = true;
    }
    if resize {
        spec.size_keyframes();
    }
    if let Some(s) = a.keyframe_scale {
        spec.keyframes.width = ((spec.width as f64 * s).round() as u32).max(16);
        spec.keyframes.height = ((spec.height as f64 * s).round() as u32).max(16);
    }
    if let Some(f) = a.keyframe_factor {
        spec.keyframes.factor = f;
    }
    if let Some(z) = a.start_zoom {
        spec.keyframes.start_zoom = z;
    }
    if a.no_auto_iterations {
        spec.keyframes.settings.auto_iterations = false;
    }
    if let Some(f) = a.fps {
        spec.fps = f;
    }
    let t = &mut spec.timing;
    if let Some(d) = a.duration {
        t.length = Length::Seconds(d);
    }
    if let Some(s) = a.per_doubling {
        t.length = Length::PerDoubling(s);
    }
    if a.fit_audio {
        t.length = Length::FitAudio;
    }
    for (value, slot) in [
        (a.ease_in, &mut t.ease_in),
        (a.ease_out, &mut t.ease_out),
        (a.hold_start, &mut t.hold_start),
        (a.hold_end, &mut t.hold_end),
    ] {
        if let Some(v) = value {
            *slot = v;
        }
    }
    if let Some(c) = a.codec {
        spec.encode.codec = c;
        // Keep the default file name's extension in step with the codec.
        if a.output.is_none() && matches!(spec.encode.output.to_str(), Some("video.mp4" | "video.mov")) {
            spec.encode.output = PathBuf::from(format!("video.{}", c.extension()));
        }
    }
    if let Some(c) = a.crf {
        spec.encode.crf = Some(c);
    }
    if let Some(o) = &a.output {
        spec.encode.output = absolute(o);
    }
    if let Some(p) = &a.audio {
        spec.encode.audio = Some(absolute(p));
    }
    if a.no_audio {
        spec.encode.audio = None;
    }
    if let Some(c) = a.cycle {
        spec.colour.cycle = c;
    }
    if !a.colour_key.is_empty() {
        spec.colour.keys = a.colour_key.iter().map(|k| parse_colour_key(k)).collect::<Result<_, _>>()?;
    }
    if a.no_colour_keys {
        spec.colour.keys.clear();
    }
    Ok(())
}

pub fn duration(s: f64) -> String {
    let s = s.max(0.0);
    if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{}m {:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{}h {:02}m", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    }
}

fn open_or_create(a: &VideoArgs) -> Result<Project, String> {
    let dir = match (&a.project, &a.location) {
        (Some(p), _) => p.clone(),
        (None, Some(loc)) => {
            let stem = loc.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "fracst".into());
            loc.with_file_name(format!("{stem}-video"))
        }
        (None, None) => return Err("give a location to zoom into, or --project to resume".into()),
    };
    if Project::exists(&dir) {
        let mut project = Project::open(&dir)?;
        if a.location.is_some() {
            println!("note: {} holds a project already; resuming it (its location is kept)", dir.display());
        }
        let mut spec = project.spec.clone();
        apply(a, &mut spec)?;
        if spec != project.spec {
            project.update(spec)?;
        }
        println!("project {} (resumed)", dir.display());
        return Ok(project);
    }
    let path = a.location.as_ref().ok_or_else(|| format!("{} holds no project; give a location", dir.display()))?;
    let location = kfr::load(path)?;
    let mut spec = VideoSpec::default();
    apply(a, &mut spec)?;
    let project = Project::create(&dir, location, spec)?;
    println!("project {} (new)", dir.display());
    Ok(project)
}

/// Ask on the terminal; `false` if there is no one to ask.
fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

fn find_ffmpeg(a: &VideoArgs) -> Result<Option<PathBuf>, String> {
    if a.frames {
        return Ok(None);
    }
    if let Some(p) = &a.ffmpeg {
        return Ok(Some(p.clone()));
    }
    if let Some(p) = ffmpeg::find() {
        return Ok(Some(p));
    }
    let Some(info) = ffmpeg::download_info() else {
        println!("ffmpeg not found, and there is no download for this platform: writing PNG frames instead");
        return Ok(None);
    };
    let question = format!(
        "ffmpeg is needed to make a video file. Download ffmpeg {} ({:.0} MB, from {}) into fracst's data folder?",
        info.version,
        info.size as f64 / 1e6,
        info.source
    );
    if !a.download_ffmpeg && !confirm(&question) {
        println!("ffmpeg not found (use --download-ffmpeg, or --ffmpeg PATH): writing PNG frames instead");
        return Ok(None);
    }
    println!("downloading {}", info.url);
    let mut last = 0;
    let path = ffmpeg::download(
        &mut |got, total| {
            let pct = got * 100 / total.max(1);
            if pct != last {
                last = pct;
                print!("\r  {pct:3}%");
                let _ = std::io::stdout().flush();
            }
        },
        &AtomicBool::new(false),
    )?;
    println!("\r  ffmpeg installed at {}", path.display());
    Ok(Some(path))
}

pub fn run(a: VideoArgs) -> Result<i32, String> {
    let project = open_or_create(&a)?;
    let spec = &project.spec;
    let plan = project.plan();
    let done = project.keyframes_done();
    println!(
        "video   {}x{} at {} fps, {} quality; {:.1} zoom doublings to {:.4}",
        spec.width,
        spec.height,
        spec.fps,
        spec.quality.name(),
        plan.doublings(),
        project.location.view.zoom
    );
    println!("frames  {} keyframes of {}x{}, {} done", plan.count, spec.keyframes.width, spec.keyframes.height, done);

    if done < plan.count {
        let engine = match &a.remote {
            Some(url) => Engine::Remote { url: url.clone(), key: crate::remote_key(&a.key) },
            None => Engine::Local,
        };
        let control = Control::default();
        let started = Instant::now();
        let mut on = |e: Event| match e {
            Event::Status(m) => println!("remote  {m}"),
            Event::Connected(engine) => println!("remote  rendering on {engine}"),
            Event::Keyframe { index, done, total, seconds } => {
                println!("key {index:5}  zoom {:.4}  {seconds:7.2}s  ({done}/{total})", plan.zoom(index))
            }
            Event::Estimate { first, remaining } => {
                println!(
                    "estimate: the first keyframe took {}; the rest take at most {}",
                    duration(first),
                    duration(remaining)
                )
            }
        };
        let limit = a.estimate.then_some(1);
        let outcome = fracst_video::render_keyframes(&project, &engine, &control, limit, &mut on)?;
        match outcome {
            Outcome::Finished => println!("keyframes done in {}", duration(started.elapsed().as_secs_f64())),
            Outcome::Limit => return Ok(0),
            Outcome::Cancelled => return Ok(1),
            Outcome::Paused(reason) => {
                eprintln!("paused: {reason}");
                eprintln!(
                    "{} of {} keyframes are saved. Resume with: fracst-cli video --project {}{}",
                    project.keyframes_done(),
                    plan.count,
                    project.dir.display(),
                    if a.remote.is_some() {
                        " [--remote URL] (without --remote it renders on this computer)"
                    } else {
                        ""
                    }
                );
                return Ok(PAUSED);
            }
        }
    } else if a.estimate {
        println!("all keyframes are rendered already");
        return Ok(0);
    }
    if a.keyframes_only || a.estimate {
        return Ok(0);
    }

    let ff = find_ffmpeg(&a)?;
    if ff.is_none() && spec.encode.audio.is_some() {
        println!("note: PNG frames have no audio");
    }
    let timeline = fracst_video::timeline(&project, ff.as_deref())?;
    let total = timeline.frames(spec.fps);
    println!("encode  {total} frames, {}", duration(timeline.total));
    let started = Instant::now();
    let mut last = Instant::now();
    let mut on = |done: usize, total: usize| {
        if last.elapsed().as_millis() >= 500 || done == total {
            last = Instant::now();
            let rate = done as f64 / started.elapsed().as_secs_f64().max(1e-9);
            print!(
                "\r  frame {done}/{total}  {rate:.1} fps  {} left   ",
                duration((total - done) as f64 / rate.max(1e-9))
            );
            let _ = std::io::stdout().flush();
        }
    };
    let (outcome, written) = fracst_video::encode(&project, ff.as_deref(), &Control::default(), &mut on)?;
    println!();
    if outcome != Outcome::Finished {
        return Ok(1);
    }
    match written {
        Written::Video(p) => println!("wrote {} in {}", p.display(), duration(started.elapsed().as_secs_f64())),
        Written::Frames(d) => println!("wrote {total} PNG frames to {}", d.display()),
    }
    Ok(0)
}
