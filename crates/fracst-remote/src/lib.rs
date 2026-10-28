//! Rendering on a remote fracst server: the protocol, frame compression
//! and a client. The server itself is the `fracst-server` crate.

pub mod client;
pub mod codec;
pub mod protocol;

pub use client::{Client, Transfer};

/// Port `fracst-server` listens on by default.
pub const DEFAULT_PORT: u16 = 7878;
