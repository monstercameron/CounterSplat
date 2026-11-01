//! Backend-independent spatial and per-entity physics components.
use serde::{Deserialize, Serialize};

/// Position for a non-camera entity. The host decides how it is rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transform {
    pub position: [f32; 3],
}
impl Default for Transform {
    fn default() -> Self {
        Self { position: [0.0; 3] }
    }
}
impl Transform {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self
            .position
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
        {
            return Err("transform position must be finite and within world bounds");
        }
        Ok(())
    }
}

/// Kinematic spherical body settings and linear motion, owned by this entity.
/// The host resolves movement against available level collision geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhysicsBody {
    pub enabled: bool,
    pub collision_enabled: bool,
    pub gravity_enabled: bool,
    /// Sphere radius in world units.
    pub radius: f32,
    /// Vertical offset from the entity origin to the collision sphere center.
    pub collision_offset_y: f32,
    /// Downward acceleration in world units per second squared.
    pub gravity: f32,
    /// World-space velocity in units per second.
    pub velocity: [f32; 3],
    /// Most recent floor-contact result from the collision host.
    pub grounded: bool,
}
impl Default for PhysicsBody {
    fn default() -> Self {
        Self {
            enabled: true,
            collision_enabled: true,
            gravity_enabled: false,
            radius: 0.2,
            collision_offset_y: 0.0,
            gravity: 18.0,
            velocity: [0.0; 3],
            grounded: false,
        }
    }
}
impl PhysicsBody {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.radius.is_finite() || !(0.05..=1.0).contains(&self.radius) {
            return Err("physics sphere radius must be between 0.05 and 1.0");
        }
        if !self.collision_offset_y.is_finite() || !(-8.0..=8.0).contains(&self.collision_offset_y)
        {
            return Err("physics collision offset must be between -8 and 8");
        }
        if !self.gravity.is_finite() || !(0.0..=100.0).contains(&self.gravity) {
            return Err("physics gravity must be between 0 and 100");
        }
        if !self
            .velocity
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 200.0)
        {
            return Err("physics velocity must be finite and between -200 and 200");
        }
        Ok(())
    }
}
