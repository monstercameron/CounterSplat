//! Asset formats shared by rendering, physics, tools and applications.
//!
//! Readers accept bytes as well as paths where the format permits it. No GPU, game state, or physics types cross
//! this boundary, so a browser host can fetch data itself and pass it to the same decoders used by native tools.

pub mod codebook;
pub mod environment;
pub mod lod;
pub mod sog;
pub mod svo;
pub mod viewer;
