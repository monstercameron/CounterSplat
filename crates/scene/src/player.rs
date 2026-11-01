//! Serializable player camera and keyboard/mouse control settings.
use serde::{Deserialize, Serialize};

/// Camera in world space. Yaw zero faces -Z, and positive pitch looks upward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov_degrees: f32,
}
impl Default for PlayerCamera {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            fov_degrees: 60.0,
        }
    }
}
impl PlayerCamera {
    /// Build a pose from a level's initial eye and target positions.
    pub fn from_view(
        position: [f32; 3],
        target: [f32; 3],
        fov_degrees: f32,
    ) -> Result<Self, &'static str> {
        let direction = [0, 1, 2].map(|i| target[i] - position[i]);
        let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !length.is_finite() || length < 0.0001 {
            return Err("camera eye and target must differ");
        }
        let camera = Self {
            position,
            yaw: direction[0].atan2(-direction[2]),
            pitch: (direction[1] / length)
                .clamp(-1.0, 1.0)
                .asin()
                .clamp(-1.553_343, 1.553_343),
            fov_degrees,
        };
        camera.validate()?;
        Ok(camera)
    }
    /// Unit view direction in world coordinates.
    pub fn direction(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [sy * cp, sp, -cy * cp]
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self
            .position
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || !self.yaw.is_finite()
            || self.yaw.abs() > 1_000_000.0
            || !self.pitch.is_finite()
            || self.pitch.abs() > 1.553_343
            || !self.fov_degrees.is_finite()
            || !(1.0..179.0).contains(&self.fov_degrees)
        {
            return Err("player camera has an invalid position, angle, or field of view");
        }
        Ok(())
    }
}

/// Player dimensions in metres and the number of level units in one metre.
/// Scan scale is not inferred; it can be calibrated independently for each level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerRig {
    pub eye_height_m: f32,
    /// Distance from the eyes to the top of the upright collider.
    pub head_clearance_m: f32,
    pub radius_m: f32,
    pub units_per_meter: f32,
    /// Align imported level-viewer spawns with nearby voxel ground when available.
    pub auto_align_spawn: bool,
}
impl Default for PlayerRig {
    fn default() -> Self {
        Self {
            eye_height_m: 1.7,
            head_clearance_m: 0.15,
            radius_m: 0.25,
            units_per_meter: 1.0,
            auto_align_spawn: true,
        }
    }
}
impl PlayerRig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.eye_height_m.is_finite()
            || !(0.7..=2.5).contains(&self.eye_height_m)
            || !self.head_clearance_m.is_finite()
            || !(0.0..=0.5).contains(&self.head_clearance_m)
            || !self.radius_m.is_finite()
            || !(0.1..=0.33).contains(&self.radius_m)
            || self.eye_height_m + self.head_clearance_m < 2.0 * self.radius_m
            || !self.units_per_meter.is_finite()
            || !(0.5..=2.5).contains(&self.units_per_meter)
        {
            return Err("invalid player eye height, radius, or level scale");
        }
        Ok(())
    }
}

/// Physical key codes, independent of keyboard character layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerBindings {
    pub forward: String,
    pub backward: String,
    pub left: String,
    pub right: String,
    pub up: String,
    pub down: String,
    pub sprint: String,
    pub jump: String,
}
impl Default for PlayerBindings {
    fn default() -> Self {
        Self {
            forward: "KeyW".into(),
            backward: "KeyS".into(),
            left: "KeyA".into(),
            right: "KeyD".into(),
            up: "KeyE".into(),
            down: "KeyQ".into(),
            sprint: "ShiftLeft".into(),
            jump: "Space".into(),
        }
    }
}
impl PlayerBindings {
    pub fn validate(&self) -> Result<(), &'static str> {
        for key in [
            &self.forward,
            &self.backward,
            &self.left,
            &self.right,
            &self.up,
            &self.down,
            &self.sprint,
            &self.jump,
        ] {
            if key.is_empty() || key.len() > 32 || !key.bytes().all(|c| c.is_ascii_alphanumeric()) {
                return Err("player key bindings must be nonempty physical key codes of at most 32 letters or digits");
            }
        }
        Ok(())
    }
}

/// Script-visible movement settings. Movement and jump speeds are in metres per second;
/// the player rig converts them to level units. Mouse sensitivity is radians per pointer pixel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerControls {
    pub enabled: bool,
    pub mouse_look: bool,
    pub move_speed: f32,
    pub sprint_multiplier: f32,
    pub mouse_sensitivity: f32,
    /// Initial upward velocity and downward acceleration for a free-fly jump, in world units.
    /// Gravity-enabled bodies use PhysicsBody.gravity for downward acceleration.
    pub jump_speed: f32,
    pub jump_gravity: f32,
    /// Optional user FOV override; None retains the level or scene camera's FOV.
    pub fov_degrees: Option<f32>,
    pub invert_y: bool,
    pub bindings: PlayerBindings,
}
impl Default for PlayerControls {
    fn default() -> Self {
        Self {
            enabled: true,
            mouse_look: true,
            move_speed: 4.0,
            sprint_multiplier: 3.0,
            mouse_sensitivity: 0.003,
            jump_speed: 5.0,
            jump_gravity: 18.0,
            fov_degrees: None,
            invert_y: false,
            bindings: PlayerBindings::default(),
        }
    }
}
impl PlayerControls {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.move_speed.is_finite()
            || !(0.1..=100.0).contains(&self.move_speed)
            || !self.sprint_multiplier.is_finite()
            || !(1.0..=10.0).contains(&self.sprint_multiplier)
            || !self.mouse_sensitivity.is_finite()
            || !(0.0001..=0.03).contains(&self.mouse_sensitivity)
            || !self.jump_speed.is_finite()
            || !(0.5..=15.0).contains(&self.jump_speed)
            || !self.jump_gravity.is_finite()
            || !(2.0..=60.0).contains(&self.jump_gravity)
            || self
                .fov_degrees
                .is_some_and(|fov| !fov.is_finite() || !(1.0..179.0).contains(&fov))
        {
            return Err("player movement, jump, or mouse sensitivity is invalid");
        }
        self.bindings.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_pose_matches_view_and_rejects_invalid_values() {
        let camera = PlayerCamera::from_view([1.0, 2.0, 3.0], [2.0, 2.0, 3.0], 60.0).unwrap();
        let direction = camera.direction();
        assert!((direction[0] - 1.0).abs() < 0.0001 && direction[2].abs() < 0.0001);
        assert!(PlayerCamera::from_view([0.0; 3], [0.0; 3], 60.0).is_err());
        assert!(PlayerCamera {
            pitch: f32::NAN,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn player_rig_validates_height_and_scale() {
        let mut rig = PlayerRig::default();
        assert!(rig.validate().is_ok());
        rig.eye_height_m = 0.1;
        assert!(rig.validate().is_err());
        rig.eye_height_m = 1.7;
        let old_saved_profile = serde_json::json!({
            "eye_height_m": 1.7, "radius_m": 0.25, "units_per_meter": 1.0, "auto_align_spawn": true
        });
        let restored: PlayerRig = serde_json::from_value(old_saved_profile).unwrap();
        assert!((restored.head_clearance_m - 0.15).abs() < 0.0001);
        rig.head_clearance_m = -0.01;
        assert!(rig.validate().is_err());
        rig.head_clearance_m = 0.15;
        rig.units_per_meter = f32::NAN;
        assert!(rig.validate().is_err());
    }
    #[test]
    fn control_configuration_rejects_invalid_inputs() {
        let mut controls = PlayerControls::default();
        assert!(controls.validate().is_ok());
        controls.mouse_sensitivity = f32::INFINITY;
        assert!(controls.validate().is_err());
        controls.mouse_sensitivity = 0.003;
        controls.jump_gravity = 0.0;
        assert!(controls.validate().is_err());
        controls.jump_gravity = 18.0;
        controls.bindings.forward = "bad key".into();
        assert!(controls.validate().is_err());
    }
}
