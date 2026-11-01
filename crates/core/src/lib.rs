//! Platform-neutral runtime building blocks.
//!
//! This crate knows nothing about GPU APIs, scan formats, physics engines, or any game. Applications own the
//! top-level loop and compose these pieces with other crates through their public APIs.

pub mod log;
pub mod pacing;
pub mod rate;
pub mod scheduler;
pub mod time;
