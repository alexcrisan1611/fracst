//! A video project: a folder holding the location (`location.kfr`), the
//! video's settings (`video.toml`) and the keyframes rendered so far
//! (`keyframes/kf_00000.fkf`, …), so that a render can stop and resume, and
//! a finished one can be encoded again with other colours or timing.
//!
//! Only [`KeyframeSpec`] decides what the keyframes look like. Everything
//! else (output size, timing, colour animation, encoding) is applied when
//! encoding and can be changed freely afterwards.

use crate::keyframe::{self, Keyframe};
use fracst_core::kfr::{self, Location};
use fracst_core::{FloatExp, Settings};
use fracst_remote::protocol::SettingsMsg;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MANIFEST: &str = "video.toml";
const LOCATION: &str = "location.kfr";
const KEYFRAMES: &str = "keyframes";

/// Quality presets: how much larger than the output the keyframes are, and
/// how many samples each output pixel takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Draft,
    #[default]
    Good,
    Best,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Draft, Quality::Good, Quality::Best];

    pub fn name(self) -> &'static str {
        match self {
            Quality::Draft => "Draft",
            Quality::Good => "Good",
            Quality::Best => "Best",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.name().eq_ignore_ascii_case(s))
    }

    /// Keyframe size relative to the output. Between keyframes the edges of
    /// the frame come from the next keyframe out, magnified up to
    /// `factor / scale` times, so at 2 nothing is ever magnified.
    pub fn keyframe_scale(self) -> f64 {
        match self {
            Quality::Draft => 1.0,
            Quality::Good => 1.5,
            Quality::Best => 2.0,
        }
    }

    /// Samples per output pixel along each axis.
    pub fn taps(self) -> u32 {
        match self {
            Quality::Draft => 1,
            Quality::Good => 2,
            Quality::Best => 3,
        }
    }
}

/// What the keyframes are: fixed once the first one is rendered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyframeSpec {
    /// Keyframe size in pixels.
    pub width: u32,
    pub height: u32,
    /// Zoom factor between neighbouring keyframes.
    pub factor: f64,
    /// The video starts at this zoom (1: the image height spans 4).
    pub start_zoom: f64,
    /// Rendering settings; the size fields are ignored.
    pub settings: SettingsMsg,
}

impl Default for KeyframeSpec {
    fn default() -> Self {
        // Each keyframe gets its own reference. Going outwards, a reused one
        // soon stops suiting the view (slow BLA, missed interior), and the
        // reference is cheap anyway when it snaps to a nearby minibrot.
        let settings = Settings { auto_iterations: true, reuse_reference: false, ..Settings::default() };
        KeyframeSpec { width: 2880, height: 1620, factor: 2.0, start_zoom: 1.0, settings: SettingsMsg::from(&settings) }
    }
}

impl KeyframeSpec {
    pub fn settings(&self) -> Settings {
        let mut s = self.settings.to_settings(u64::MAX).unwrap_or_default();
        s.width = self.width;
        s.height = self.height;
        s
    }
}

/// How long the zoom takes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Length {
    /// The whole video, holds included, lasts this many seconds.
    Seconds(f64),
    /// Each doubling of the zoom takes this many seconds at full speed.
    PerDoubling(f64),
    /// The whole video lasts as long as the audio track.
    FitAudio,
}

/// Pacing: the zoom accelerates for `ease_in` seconds, runs at constant
/// speed (in zoom doublings per second), and decelerates for `ease_out`
/// seconds, with still frames held at the start and the end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timing {
    pub length: Length,
    pub ease_in: f64,
    pub ease_out: f64,
    pub hold_start: f64,
    pub hold_end: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { length: Length::PerDoubling(1.0), ease_in: 2.0, ease_out: 4.0, hold_start: 0.0, hold_end: 2.0 }
    }
}

/// Colour settings at a moment of the video. Each field is interpolated
/// separately between the keys that set it; unset fields follow the others.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ColourKey {
    /// Seconds from the start of the video.
    pub time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iter_div: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slope_power: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slope_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slope_angle: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourAnim {
    /// Palette cycling: the colour offset advances this many palette
    /// entries (of 1024) per second.
    pub cycle: f64,
    pub keys: Vec<ColourKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    #[default]
    H264,
    H265,
    Av1,
    #[serde(rename = "prores")]
    ProRes,
}

impl Codec {
    pub const ALL: [Codec; 4] = [Codec::H264, Codec::H265, Codec::Av1, Codec::ProRes];

    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::H265 => "H.265",
            Codec::Av1 => "AV1",
            Codec::ProRes => "ProRes",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.to_ascii_lowercase().replace(['.', '-'], "");
        match s.as_str() {
            "h264" | "avc" | "x264" => Some(Codec::H264),
            "h265" | "hevc" | "x265" => Some(Codec::H265),
            "av1" => Some(Codec::Av1),
            "prores" => Some(Codec::ProRes),
            _ => None,
        }
    }

    /// File extension of the container this codec goes in.
    pub fn extension(self) -> &'static str {
        if self == Codec::ProRes { "mov" } else { "mp4" }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncodeSpec {
    /// The video file; relative paths are inside the project folder.
    pub output: PathBuf,
    pub codec: Codec,
    /// Quality for the codec's constant-quality mode (lower is better);
    /// unset picks a codec-specific default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crf: Option<u32>,
    /// Music or sound to put under the video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<PathBuf>,
}

impl Default for EncodeSpec {
    fn default() -> Self {
        EncodeSpec { output: PathBuf::from("video.mp4"), codec: Codec::H264, crf: None, audio: None }
    }
}

/// Everything in `video.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSpec {
    /// Output size in pixels (even, for the usual codecs).
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub quality: Quality,
    pub keyframes: KeyframeSpec,
    pub timing: Timing,
    pub colour: ColourAnim,
    pub encode: EncodeSpec,
}

impl Default for VideoSpec {
    fn default() -> Self {
        let mut spec = VideoSpec {
            width: 1920,
            height: 1080,
            fps: 60.0,
            quality: Quality::Good,
            keyframes: KeyframeSpec::default(),
            timing: Timing::default(),
            colour: ColourAnim::default(),
            encode: EncodeSpec::default(),
        };
        spec.size_keyframes();
        spec
    }
}

impl VideoSpec {
    /// Size the keyframes for the output size and quality.
    pub fn size_keyframes(&mut self) {
        let s = self.quality.keyframe_scale();
        self.keyframes.width = ((self.width as f64 * s).round() as u32).max(16);
        self.keyframes.height = ((self.height as f64 * s).round() as u32).max(16);
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.width < 16 || self.height < 16 || !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err(format!("the video size {}x{} must be even and at least 16x16", self.width, self.height));
        }
        if !(self.fps > 0.0 && self.fps <= 240.0) {
            return Err("the frame rate must be between 0 and 240".into());
        }
        let k = &self.keyframes;
        if k.width < 16 || k.height < 16 {
            return Err("keyframes must be at least 16x16".into());
        }
        if !(k.factor > 1.0 && k.factor <= 16.0) {
            return Err("the keyframe zoom factor must be between 1 and 16".into());
        }
        if !(k.start_zoom > 0.0) {
            return Err("the start zoom must be positive".into());
        }
        let aspect = |w: u32, h: u32| w as f64 / h as f64;
        if (aspect(self.width, self.height) / aspect(k.width, k.height) - 1.0).abs() > 0.02 {
            return Err(format!(
                "the video ({}x{}) and its keyframes ({}x{}) have different shapes",
                self.width, self.height, k.width, k.height
            ));
        }
        let t = &self.timing;
        if [t.ease_in, t.ease_out, t.hold_start, t.hold_end].iter().any(|v| !(*v >= 0.0)) {
            return Err("easing and hold times cannot be negative".into());
        }
        match t.length {
            Length::Seconds(s) if !(s > t.hold_start + t.hold_end) => {
                Err("the video must be longer than its holds at the start and end".into())
            }
            Length::PerDoubling(s) if !(s > 0.0) => Err("seconds per zoom doubling must be positive".into()),
            Length::FitAudio if self.encode.audio.is_none() => {
                Err("fitting the length to the audio needs an audio track".into())
            }
            _ => Ok(()),
        }
    }
}

/// Which keyframes there are: keyframe `k` is at zoom `end / factor^k`,
/// from the location itself (`k = 0`) out to past the start zoom.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub end_zoom: FloatExp,
    pub factor: f64,
    pub count: usize,
    /// `log2(start_zoom / end_zoom)`: where the video starts, at most 0.
    pub start_log2: f64,
}

impl Plan {
    pub fn new(end_zoom: FloatExp, k: &KeyframeSpec) -> Result<Plan, String> {
        let start_log2 = k.start_zoom.log2() - end_zoom.log2();
        let doublings = -start_log2;
        if doublings < k.factor.log2() {
            return Err(format!(
                "the location (zoom {end_zoom:.3}) is too shallow for a zoom video from zoom {}",
                k.start_zoom
            ));
        }
        // The frame at the start zoom still needs a keyframe that covers it
        // by `factor` all round.
        let count = (doublings / k.factor.log2() - 1e-9).ceil() as usize + 2;
        Ok(Plan { end_zoom, factor: k.factor, count, start_log2 })
    }

    /// `log2(zoom_k / end_zoom)`.
    pub fn log2(&self, k: usize) -> f64 {
        -(k as f64) * self.factor.log2()
    }

    pub fn zoom(&self, k: usize) -> FloatExp {
        self.end_zoom * FloatExp::exp2(self.log2(k))
    }

    /// Zoom doublings from the start to the location.
    pub fn doublings(&self) -> f64 {
        -self.start_log2
    }
}

pub struct Project {
    pub dir: PathBuf,
    pub spec: VideoSpec,
    pub location: Location,
}

impl Project {
    /// Is there a project in `dir`?
    pub fn exists(dir: &Path) -> bool {
        dir.join(MANIFEST).is_file()
    }

    /// Start a project in `dir` (which may exist, but not hold a project).
    pub fn create(dir: &Path, location: Location, spec: VideoSpec) -> Result<Project, String> {
        spec.validate()?;
        Plan::new(location.view.zoom, &spec.keyframes)?;
        if Self::exists(dir) {
            return Err(format!("{} already holds a video project", dir.display()));
        }
        std::fs::create_dir_all(dir.join(KEYFRAMES)).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        kfr::save(&dir.join(LOCATION), &location.view, &location.colouring)?;
        let project = Project { dir: dir.to_path_buf(), spec, location };
        project.save()?;
        Ok(project)
    }

    pub fn open(dir: &Path) -> Result<Project, String> {
        let text = std::fs::read_to_string(dir.join(MANIFEST))
            .map_err(|e| format!("{} is not a video project: {e}", dir.display()))?;
        let spec: VideoSpec = toml::from_str(&text).map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))?;
        let location = kfr::load(&dir.join(LOCATION))?;
        std::fs::create_dir_all(dir.join(KEYFRAMES)).map_err(|e| e.to_string())?;
        Ok(Project { dir: dir.to_path_buf(), spec, location })
    }

    /// Write `video.toml`.
    pub fn save(&self) -> Result<(), String> {
        self.spec.validate()?;
        let text = toml::to_string_pretty(&self.spec).map_err(|e| e.to_string())?;
        let header = "# fracst video project. Keyframes depend only on [keyframes]; the rest\n\
                      # applies when encoding and can be changed at any time.\n\n";
        std::fs::write(self.dir.join(MANIFEST), format!("{header}{text}")).map_err(|e| e.to_string())
    }

    /// Change the settings. Keyframe settings can only change while no
    /// keyframe has been rendered.
    pub fn update(&mut self, spec: VideoSpec) -> Result<(), String> {
        spec.validate()?;
        if spec.keyframes != self.spec.keyframes && self.keyframes_done() > 0 {
            return Err("the keyframe settings (size, quality, zoom factor, rendering) cannot change once \
                        keyframes are rendered; start a new project for that"
                .into());
        }
        Plan::new(self.location.view.zoom, &spec.keyframes)?;
        self.spec = spec;
        self.save()
    }

    pub fn plan(&self) -> Plan {
        Plan::new(self.location.view.zoom, &self.spec.keyframes).expect("validated when created")
    }

    pub fn keyframe_path(&self, k: usize) -> PathBuf {
        self.dir.join(KEYFRAMES).join(format!("kf_{k:05}.fkf"))
    }

    pub fn has_keyframe(&self, k: usize) -> bool {
        self.keyframe_path(k).is_file()
    }

    pub fn keyframes_done(&self) -> usize {
        (0..self.plan().count).filter(|&k| self.has_keyframe(k)).count()
    }

    pub fn load_keyframe(&self, k: usize) -> Result<Keyframe, String> {
        keyframe::load(&self.keyframe_path(k))
    }

    /// The video file, resolved against the project folder.
    pub fn output_path(&self) -> PathBuf {
        self.dir.join(&self.spec.encode.output)
    }

    /// Where a PNG sequence goes when there is no ffmpeg.
    pub fn frames_dir(&self) -> PathBuf {
        self.dir.join("frames")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let mut spec = VideoSpec::default();
        spec.timing.length = Length::Seconds(42.0);
        spec.colour.keys.push(ColourKey { time: 3.0, offset: Some(100.0), ..Default::default() });
        spec.encode.audio = Some("song.flac".into());
        let text = toml::to_string_pretty(&spec).unwrap();
        assert_eq!(toml::from_str::<VideoSpec>(&text).unwrap(), spec);
        spec.timing.length = Length::FitAudio;
        let text = toml::to_string_pretty(&spec).unwrap();
        assert_eq!(toml::from_str::<VideoSpec>(&text).unwrap(), spec);
        // Missing fields take defaults.
        assert_eq!(toml::from_str::<VideoSpec>("fps = 30.0").unwrap().width, 1920);
    }

    #[test]
    fn plan_covers_the_start() {
        let k = KeyframeSpec::default();
        let plan = Plan::new(FloatExp::exp2(100.0), &k).unwrap();
        assert_eq!(plan.count, 102);
        // The last keyframe is at least one factor further out than the start.
        assert!(plan.log2(plan.count - 1) <= plan.start_log2 - 1.0);
        assert!(Plan::new(FloatExp::from(1.5), &k).is_err());
    }
}
