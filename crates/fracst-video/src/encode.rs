//! Making the video: every frame composited from the keyframes and piped
//! into ffmpeg, or written as a PNG sequence when there is no ffmpeg.

use crate::compose::Compositor;
use crate::ffmpeg::{self, EncodeJob, Encoder};
use crate::project::{Length, Project};
use crate::render::{Control, Outcome};
use crate::timeline::Timeline;
use fracst_core::image;
use std::path::{Path, PathBuf};

/// The project's timeline. Reading the audio's length needs ffmpeg.
pub fn timeline(project: &Project, ffmpeg: Option<&Path>) -> Result<Timeline, String> {
    let audio = match (&project.spec.encode.audio, ffmpeg) {
        (Some(a), Some(ff)) => Some(ffmpeg::duration(ff, &project.dir.join(a))?),
        (Some(_), None) if project.spec.timing.length == Length::FitAudio => {
            return Err("fitting the length to the audio needs ffmpeg".into());
        }
        _ => None,
    };
    Timeline::new(&project.spec.timing, &project.plan(), audio)
}

/// Where the frames went.
pub enum Written {
    Video(PathBuf),
    /// PNG files in this folder.
    Frames(PathBuf),
}

/// Encode the video (with `ffmpeg`), or write PNG frames (without).
/// `on` gets frames done and the total.
pub fn encode(
    project: &Project,
    ffmpeg: Option<&Path>,
    control: &Control,
    on: &mut dyn FnMut(usize, usize),
) -> Result<(Outcome, Written), String> {
    let plan = project.plan();
    if let Some(k) = (0..plan.count).find(|&k| !project.has_keyframe(k)) {
        return Err(format!("keyframe {k} of {} is not rendered yet", plan.count));
    }
    let spec = &project.spec;
    let timeline = timeline(project, ffmpeg)?;
    let total = timeline.frames(spec.fps);
    let mut compositor = Compositor::new(project);
    let taps = spec.quality.taps();
    let frame = |c: &mut Compositor, i: usize| c.frame(&timeline, i as f64 / spec.fps, spec.width, spec.height, taps);

    let Some(ff) = ffmpeg else {
        let dir = project.frames_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        for i in 0..total {
            if control.stopped() {
                return Ok((Outcome::Cancelled, Written::Frames(dir)));
            }
            let rgba = frame(&mut compositor, i)?;
            image::save_png(&dir.join(format!("frame_{i:06}.png")), &rgba, spec.width, spec.height, None)?;
            on(i + 1, total);
        }
        return Ok((Outcome::Finished, Written::Frames(dir)));
    };

    let output = project.output_path();
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let audio_path = spec.encode.audio.as_ref().map(|a| project.dir.join(a));
    let audio = match &audio_path {
        Some(p) => Some((p.as_path(), ffmpeg::duration(ff, p)?)),
        None => None,
    };
    let job = EncodeJob {
        ffmpeg: ff,
        output: &output,
        width: spec.width,
        height: spec.height,
        fps: spec.fps,
        codec: spec.encode.codec,
        crf: spec.encode.crf,
        audio,
        length: total as f64 / spec.fps,
    };
    let mut encoder = Encoder::start(&job)?;
    for i in 0..total {
        if control.stopped() {
            encoder.abort();
            let _ = std::fs::remove_file(&output);
            return Ok((Outcome::Cancelled, Written::Video(output)));
        }
        let rgba = match frame(&mut compositor, i) {
            Ok(f) => f,
            Err(e) => {
                encoder.abort();
                return Err(e);
            }
        };
        encoder.write_rgba(&rgba)?;
        on(i + 1, total);
    }
    encoder.finish()?;
    Ok((Outcome::Finished, Written::Video(output)))
}
