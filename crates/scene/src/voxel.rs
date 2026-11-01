//! Script-visible voxel collider controls and worker state.
use serde::{Deserialize, Serialize};

/// Independent switches for the level voxel system, debug view and player collision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoxelCollider {
    pub enabled: bool,
    pub visible: bool,
    pub physics: bool,
    /// Legacy scene field; runtime collider sizes now come from each entity's PhysicsBody.
    pub player_radius: f32,
}
impl Default for VoxelCollider {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: false,
            physics: true,
            player_radius: 0.2,
        }
    }
}
impl VoxelCollider {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.player_radius.is_finite() || !(0.05..=1.0).contains(&self.player_radius) {
            return Err("voxel player radius must be finite and between 0.05 and 1.0");
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoxelStatus {
    #[default]
    Loading,
    Ready,
    Unavailable,
    Error,
}
/// A small host-owned status snapshot, not the voxel data itself.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VoxelColliderState {
    pub status: VoxelStatus,
    pub resolution: Option<f32>,
    pub visible_voxels: u32,
    pub message: String,
}
impl VoxelColliderState {
    pub fn validate(&self) -> Result<(), &'static str> {
        if (self.status == VoxelStatus::Ready && self.resolution.is_none())
            || self
                .resolution
                .is_some_and(|n| !n.is_finite() || n <= 0.0 || n > 1000.0)
            || self.visible_voxels > 10_000
            || self.message.len() > 256
            || self.message.chars().any(char::is_control)
        {
            return Err("invalid voxel collider status");
        }
        Ok(())
    }
}
