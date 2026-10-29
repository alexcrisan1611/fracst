//! Messages between a fracst client and `fracst-server`, as JSON text
//! messages over a WebSocket. A frame is a [`ServerMsg::Frame`] header
//! followed by `blocks` binary messages (see [`crate::codec`]).
//!
//! Views travel exactly (the centre's limbs and the zoom's mantissa and
//! exponent), so a remote frame is identical to one rendered locally.

use fracst_core::{Fixed, FloatExp, Settings, Stats, View};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Bumped on incompatible changes.
pub const PROTOCOL: u32 = 1;

/// The server accepts queued render requests (see [`ClientMsg::Render`]).
pub const FEATURE_QUEUE: &str = "queue";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Render a frame. A newer request cancels the one in flight, unless it
    /// is queued: then it waits its turn and frames come back in order.
    /// Only for servers whose hello lists [`FEATURE_QUEUE`].
    Render {
        id: u64,
        view: ViewMsg,
        settings: SettingsMsg,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        queue: bool,
    },
    Cancel {
        id: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Sent once on connecting.
    Hello {
        protocol: u32,
        version: String,
        engine: String,
        /// Optional abilities, such as [`FEATURE_QUEUE`].
        #[serde(default)]
        features: Vec<String>,
    },
    /// From a gateway before the hello, while it gets a server ready (for
    /// example a GPU starting up). For display.
    Status {
        message: String,
    },
    /// Sent every so often on an idle connection, so that proxies on the
    /// way do not drop it. Clients ignore it.
    Keepalive,
    /// Progress of a render in flight: stage (see `fracst_core::Progress`) and fraction done.
    Progress {
        id: u64,
        stage: u8,
        fraction: f64,
    },
    /// A finished frame; its data follows in `blocks` binary messages.
    Frame(FrameHeader),
    Error {
        id: Option<u64>,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameHeader {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub maxiter: u64,
    /// Whether the data includes distance estimates.
    pub de: bool,
    /// Number of binary messages that follow.
    pub blocks: u32,
    /// Total size of those messages.
    pub bytes: u64,
    pub stats: StatsMsg,
    /// Seconds spent rendering and compressing on the server.
    pub render_s: f64,
    pub encode_s: f64,
}

/// A [`View`], exactly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewMsg {
    /// The centre's limbs in hex, least significant first (see `Fixed::as_limbs`).
    pub re: Vec<String>,
    pub im: Vec<String>,
    /// Mantissa and binary exponent.
    pub zoom: (f64, i32),
    pub maxiter: u64,
    pub imag_down: bool,
}

fn limbs_to_hex(f: &Fixed) -> Vec<String> {
    f.as_limbs().iter().map(|l| format!("{l:016x}")).collect()
}

fn hex_to_fixed(limbs: &[String]) -> Result<Fixed, String> {
    let limbs = limbs.iter().map(|l| u64::from_str_radix(l, 16)).collect::<Result<Vec<_>, _>>();
    Fixed::from_limbs(limbs.map_err(|e| format!("bad limb: {e}"))?).ok_or_else(|| "too few limbs".into())
}

impl From<&View> for ViewMsg {
    fn from(v: &View) -> Self {
        ViewMsg {
            re: limbs_to_hex(&v.re),
            im: limbs_to_hex(&v.im),
            zoom: v.zoom.parts(),
            maxiter: v.maxiter,
            imag_down: v.imag_down,
        }
    }
}

impl ViewMsg {
    pub fn to_view(&self) -> Result<View, String> {
        let (re, im) = (hex_to_fixed(&self.re)?, hex_to_fixed(&self.im)?);
        if re.limbs() != im.limbs() {
            return Err("re and im have different precision".into());
        }
        let zoom = FloatExp::new(self.zoom.0, self.zoom.1);
        if !(self.zoom.0.is_finite() && self.zoom.0 > 0.0) {
            return Err("zoom must be positive".into());
        }
        Ok(View { re, im, zoom, maxiter: self.maxiter, imag_down: self.imag_down })
    }
}

/// [`Settings`]; missing fields take their defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsMsg {
    pub width: u32,
    pub height: u32,
    pub bailout: f64,
    pub bla: bool,
    pub bla_epsilon: f64,
    pub distance_estimate: bool,
    pub interior_detection: bool,
    pub reuse_reference: bool,
    pub auto_iterations: bool,
    pub max_iterations: u64,
    pub periodic_reference: bool,
    pub simd: bool,
}

impl Default for SettingsMsg {
    fn default() -> Self {
        SettingsMsg::from(&Settings::default())
    }
}

impl From<&Settings> for SettingsMsg {
    fn from(s: &Settings) -> Self {
        SettingsMsg {
            width: s.width,
            height: s.height,
            bailout: s.bailout,
            bla: s.bla,
            bla_epsilon: s.bla_epsilon,
            distance_estimate: s.distance_estimate,
            interior_detection: s.interior_detection,
            reuse_reference: s.reuse_reference,
            auto_iterations: s.auto_iterations,
            max_iterations: s.max_iterations,
            periodic_reference: s.periodic_reference,
            simd: s.simd,
        }
    }
}

impl SettingsMsg {
    /// The settings, with the image size capped at `max_pixels`.
    pub fn to_settings(&self, max_pixels: u64) -> Result<Settings, String> {
        let pixels = self.width as u64 * self.height as u64;
        if self.width == 0 || self.height == 0 || pixels > max_pixels {
            return Err(format!("image size {}x{} is outside 1..={max_pixels} pixels", self.width, self.height));
        }
        Ok(Settings {
            width: self.width,
            height: self.height,
            bailout: self.bailout,
            bla: self.bla,
            bla_epsilon: self.bla_epsilon,
            distance_estimate: self.distance_estimate,
            interior_detection: self.interior_detection,
            reuse_reference: self.reuse_reference,
            auto_iterations: self.auto_iterations,
            max_iterations: self.max_iterations,
            periodic_reference: self.periodic_reference,
            simd: self.simd,
        })
    }
}

/// [`Stats`], with durations in seconds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsMsg {
    pub reference: f64,
    pub reference_reused: bool,
    pub reference_len: usize,
    pub reference_escaped: bool,
    pub reference_truncated: bool,
    pub reference_limbs: usize,
    pub nearby_period: Option<u64>,
    pub reference_on_nucleus: bool,
    pub bla: f64,
    pub bla_levels: usize,
    pub pixels: f64,
    pub total: f64,
    pub iterations: u64,
    pub steps: u64,
    pub deep_steps: u64,
    pub interior: u64,
    pub undecided: u64,
    pub min_escaped: u64,
    pub max_escaped: u64,
    pub rounds: u32,
    pub suggested_maxiter: u64,
    pub mode: String,
}

impl From<&Stats> for StatsMsg {
    fn from(s: &Stats) -> Self {
        StatsMsg {
            reference: s.reference.as_secs_f64(),
            reference_reused: s.reference_reused,
            reference_len: s.reference_len,
            reference_escaped: s.reference_escaped,
            reference_truncated: s.reference_truncated,
            reference_limbs: s.reference_limbs,
            nearby_period: s.nearby_period,
            reference_on_nucleus: s.reference_on_nucleus,
            bla: s.bla.as_secs_f64(),
            bla_levels: s.bla_levels,
            pixels: s.pixels.as_secs_f64(),
            total: s.total.as_secs_f64(),
            iterations: s.iterations,
            steps: s.steps,
            deep_steps: s.deep_steps,
            interior: s.interior,
            undecided: s.undecided,
            min_escaped: s.min_escaped,
            max_escaped: s.max_escaped,
            rounds: s.rounds,
            suggested_maxiter: s.suggested_maxiter,
            mode: s.mode.to_string(),
        }
    }
}

impl StatsMsg {
    pub fn to_stats(&self) -> Stats {
        let secs = |s: f64| Duration::try_from_secs_f64(s).unwrap_or_default();
        Stats {
            reference: secs(self.reference),
            reference_reused: self.reference_reused,
            reference_len: self.reference_len,
            reference_escaped: self.reference_escaped,
            reference_truncated: self.reference_truncated,
            reference_limbs: self.reference_limbs,
            nearby_period: self.nearby_period,
            reference_on_nucleus: self.reference_on_nucleus,
            bla: secs(self.bla),
            bla_levels: self.bla_levels,
            pixels: secs(self.pixels),
            total: secs(self.total),
            iterations: self.iterations,
            steps: self.steps,
            deep_steps: self.deep_steps,
            interior: self.interior,
            undecided: self.undecided,
            min_escaped: self.min_escaped,
            max_escaped: self.max_escaped,
            rounds: self.rounds,
            suggested_maxiter: self.suggested_maxiter,
            mode: match self.mode.as_str() {
                "f64" => "f64",
                "floatexp+f64" => "floatexp+f64",
                _ => "remote",
            },
        }
    }
}
