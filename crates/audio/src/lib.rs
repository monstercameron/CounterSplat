//! Layered, stereo-aware audio without dependencies on the rest of the engine.
//!
//! [`AudioMixer`] owns mixer state on the calling thread. Kira/CPAL owns the real-time
//! device callback; the host can use its bounded subsystem worker to send [`AudioCommand`]
//! values to this owner without sharing the mixer or blocking the audio callback.
//! [`OfflineMixer`] uses the same mixing path for deterministic tests and rendering.
//!
//! Clips accept mono or stereo; mono is duplicated, stereo channels remain distinct, and
//! multichannel input is rejected rather than silently misinterpreted. Gain is linear and
//! balance ranges from -1 (left) to 1 (right). Mixer output can clip when layers sum above
//! full scale; leave headroom when combining loud sounds.

mod clip;
mod command;
mod components;
mod config;
mod error;
mod mixer;
mod offline;

pub use clip::{AudioClip, StereoFrame, MAX_CLIP_FRAMES, MAX_ENCODED_BYTES};
pub use command::{AudioCommand, AudioReply};
pub use components::{AudioEmitter, AudioLayer};
pub use config::{AudioConfig, LayerMix, PlaySettings, DEFAULT_LAYERS};
pub use error::AudioError;
#[cfg(feature = "device")]
pub use mixer::AudioMixer;
pub use mixer::{Mixer, VoiceId, VoiceStatus};
pub use offline::{OfflineBackend, OfflineMixer};
