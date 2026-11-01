//! Game-neutral rigid-body and scan-collision building blocks.
//!
//! Applications choose their own actors and control schemes. This crate owns the Rapier integration and accepts
//! asset data through `countersplat-assets`; it does not depend on rendering or game-specific concepts.

pub mod floor;
pub mod voxel;
pub mod voxel_collision;
pub mod world;
