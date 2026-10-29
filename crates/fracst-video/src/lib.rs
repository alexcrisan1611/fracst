//! Zoom videos: a sequence of keyframes rendered from the location outwards
//! ([`render`]), stored as raw iteration data ([`keyframe`]) in a resumable
//! project folder ([`project`]), then composited into smooth frames
//! ([`compose`]) along a speed curve with animated colours ([`timeline`])
//! and encoded with ffmpeg ([`ffmpeg`], [`encode`]).

// `!(a > b)` is used on purpose: it rejects NaN as well.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod compose;
pub mod encode;
pub mod ffmpeg;
pub mod keyframe;
pub mod project;
pub mod render;
pub mod timeline;

pub use compose::Compositor;
pub use encode::{Written, encode, timeline};
pub use project::{Codec, ColourAnim, ColourKey, Length, Plan, Project, Quality, Timing, VideoSpec};
pub use render::{Control, Engine, Event, Outcome, render_keyframes};
pub use timeline::{Timeline, colouring_at};
