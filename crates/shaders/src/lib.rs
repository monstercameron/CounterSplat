//! Gaussian-splat shader composition and the embedded upstream WGSL catalogue.
//!
//! GPU-independent source processing lives here. Render backends consume composed source and binding descriptions;
//! they never include files from this crate by relative path. See `NOTICE.md` for source attribution.

pub mod binding;
pub mod declarations;
pub mod pipeline;
pub mod preprocess;
pub mod radix;

include!(concat!(env!("OUT_DIR"), "/chunks.rs"));

/// Returns the upstream WGSL for an include key such as `gsplatCenterVS`.
pub fn chunk_by_key(key: &str) -> Option<&'static str> {
    let (_, path) = KEYS.iter().find(|(k, _)| *k == key)?;
    chunk_by_path(path)
}

/// Returns upstream WGSL by its path relative to this crate's `shaders/` directory.
pub fn chunk_by_path(path: &str) -> Option<&'static str> {
    CHUNKS.iter().find(|(p, _)| *p == path).map(|(_, s)| *s)
}
