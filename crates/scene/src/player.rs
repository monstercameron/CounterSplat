//! Serializable player camera and keyboard/mouse control settings.
use serde::{Deserialize, Serialize};

/// Camera in world space. Yaw zero faces -Z, and positive pitch looks upward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerCamera {
    pub position: [f32; 3],
    /// World-space visual offset. Physics and the player position remain unchanged.
    pub view_offset: [f32; 3],
    /// Temporary visual smoothing of an already collision-checked posture change.
    pub view_posture_offset_y: f32,
    /// Independent visual-only gait and turn offsets, in radians.
    pub view_bob_pitch: f32,
    pub view_bob_roll: f32,
    pub view_turn_roll: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// Roll around the viewing axis, in radians.
    pub roll: f32,
    pub fov_degrees: f32,
    /// Optional script-controlled render FOV; the configured base FOV stays unchanged.
    pub view_fov_degrees: Option<f32>,
}
impl Default for PlayerCamera {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            view_offset: [0.0, 0.0, 0.0],
            view_posture_offset_y: 0.0,
            view_bob_pitch: 0.0,
            view_bob_roll: 0.0,
            view_turn_roll: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov_degrees: 60.0,
            view_fov_degrees: None,
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
            view_offset: [0.0; 3],
            view_posture_offset_y: 0.0,
            view_bob_pitch: 0.0,
            view_bob_roll: 0.0,
            view_turn_roll: 0.0,
            yaw: direction[0].atan2(-direction[2]),
            pitch: (direction[1] / length)
                .clamp(-1.0, 1.0)
                .asin()
                .clamp(-1.553_343, 1.553_343),
            roll: 0.0,
            fov_degrees,
            view_fov_degrees: None,
        };
        camera.validate()?;
        Ok(camera)
    }
    /// Visual eye position; collision and gameplay use `position` directly.
    pub fn render_position(&self) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.position[i]
                + self.view_offset[i]
                + if i == 1 {
                    self.view_posture_offset_y
                } else {
                    0.0
                }
        })
    }

    /// Render FOV in degrees, including any script-controlled lens effect.
    pub fn render_fov_degrees(&self) -> f32 {
        self.view_fov_degrees.unwrap_or(self.fov_degrees)
    }

    fn direction_at(&self, pitch: f32) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        [sy * cp, sp, -cy * cp]
    }
    fn up_at(&self, pitch: f32, roll: f32) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let (sr, cr) = roll.sin_cos();
        let right = [cy, 0.0, sy];
        let up = [-sy * sp, cp, cy * sp];
        std::array::from_fn(|i| up[i] * cr + right[i] * sr)
    }
    /// Gameplay aim excludes visual camera effects.
    pub fn direction(&self) -> [f32; 3] {
        self.direction_at(self.pitch)
    }
    /// Authored camera up, excluding visual-only motion.
    pub fn up(&self) -> [f32; 3] {
        self.up_at(self.pitch, self.roll)
    }
    /// View direction including gait pitch; never changes gameplay aim.
    pub fn render_direction(&self) -> [f32; 3] {
        self.direction_at((self.pitch + self.view_bob_pitch).clamp(-1.553_343, 1.553_343))
    }
    /// View up including independently composable gait and turning rolls.
    pub fn render_up(&self) -> [f32; 3] {
        self.up_at(
            (self.pitch + self.view_bob_pitch).clamp(-1.553_343, 1.553_343),
            self.roll + self.view_bob_roll + self.view_turn_roll,
        )
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self
            .position
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || !self
                .view_offset
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 100.0)
            || !self.view_posture_offset_y.is_finite()
            || self.view_posture_offset_y.abs() > 100.0
            || ![self.view_bob_pitch, self.view_bob_roll, self.view_turn_roll]
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 0.25)
            || !self.yaw.is_finite()
            || self.yaw.abs() > 1_000_000.0
            || !self.pitch.is_finite()
            || self.pitch.abs() > 1.553_343
            || !self.roll.is_finite()
            || self.roll.abs() > std::f32::consts::PI
            || !self.fov_degrees.is_finite()
            || !(1.0..179.0).contains(&self.fov_degrees)
            || self
                .view_fov_degrees
                .is_some_and(|fov| !fov.is_finite() || !(0.01..179.0).contains(&fov))
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
    pub crouch: String,
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
            crouch: "ControlLeft".into(),
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
            &self.crouch,
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
    /// Lua camera bob settings: steps per second at walking speed, amplitudes in metres,
    /// and response in inverse seconds. The view offset never moves the collider.
    pub bob_enabled: bool,
    pub bob_cadence: f32,
    pub bob_vertical: f32,
    pub bob_horizontal: f32,
    pub bob_smoothing: f32,
    /// Right-button optical magnification and transition response in inverse seconds.
    pub zoom_factor: f32,
    pub zoom_response: f32,
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
            bob_enabled: true,
            bob_cadence: 2.0,
            bob_vertical: 0.035,
            bob_horizontal: 0.018,
            bob_smoothing: 12.0,
            zoom_factor: 5.0,
            zoom_response: 12.0,
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
            || !self.bob_cadence.is_finite()
            || !(0.2..=6.0).contains(&self.bob_cadence)
            || !self.bob_vertical.is_finite()
            || !(0.0..=0.25).contains(&self.bob_vertical)
            || !self.bob_horizontal.is_finite()
            || !(0.0..=0.25).contains(&self.bob_horizontal)
            || !self.bob_smoothing.is_finite()
            || !(1.0..=30.0).contains(&self.bob_smoothing)
            || !self.zoom_factor.is_finite()
            || !(1.0..=10.0).contains(&self.zoom_factor)
            || !self.zoom_response.is_finite()
            || !(1.0..=30.0).contains(&self.zoom_response)
            || self
                .fov_degrees
                .is_some_and(|fov| !fov.is_finite() || !(1.0..179.0).contains(&fov))
        {
            return Err("player movement, camera, or mouse settings are invalid");
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
    fn camera_roll_changes_up_without_turning_or_moving_the_camera() {
        let mut camera = PlayerCamera {
            yaw: 0.7,
            pitch: 0.3,
            ..Default::default()
        };
        let direction = camera.direction();
        let original_up = camera.up();
        camera.roll = std::f32::consts::FRAC_PI_2;
        let up = camera.up();
        assert_eq!(camera.direction(), direction);
        assert!((up.iter().zip(direction).map(|(a, b)| a * b).sum::<f32>()).abs() < 1e-6);
        assert!((up.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((up.iter().zip(original_up).map(|(a, b)| a * b).sum::<f32>()).abs() < 1e-6);
        let old: PlayerCamera = serde_json::from_str("{}").unwrap();
        assert_eq!(old.roll, 0.0);
    }
    #[test]
    fn visual_rotation_preserves_aim_and_keeps_the_render_basis_orthogonal() {
        let mut camera = PlayerCamera::default();
        let aim = camera.direction();
        camera.view_bob_pitch = 0.05;
        camera.view_bob_roll = 0.02;
        camera.view_turn_roll = -0.03;
        assert_eq!(camera.direction(), aim);
        assert_ne!(camera.render_direction(), aim);
        let direction = camera.render_direction();
        let up = camera.render_up();
        assert!(
            direction
                .iter()
                .zip(up)
                .map(|(a, b)| a * b)
                .sum::<f32>()
                .abs()
                < 1e-6
        );
        assert!(camera.validate().is_ok());
        camera.view_turn_roll = f32::NAN;
        assert!(camera.validate().is_err());
    }
    #[test]
    fn camera_view_offset_is_visual_only_and_old_settings_get_defaults() {
        let camera = PlayerCamera {
            position: [1.0, 2.0, 3.0],
            view_offset: [0.1, 0.2, -0.3],
            ..Default::default()
        };
        assert_eq!(camera.render_position(), [1.1, 2.2, 2.7]);
        let crouch = PlayerCamera {
            view_posture_offset_y: 0.3,
            ..camera.clone()
        };
        assert!((crouch.render_position()[1] - 2.5).abs() < 1e-6);
        assert_eq!(crouch.position, camera.position);
        assert!(PlayerCamera {
            view_posture_offset_y: f32::NAN,
            ..camera.clone()
        }
        .validate()
        .is_err());
        assert_eq!(camera.position, [1.0, 2.0, 3.0]);
        assert_eq!(camera.render_fov_degrees(), camera.fov_degrees);
        let zoomed = PlayerCamera {
            view_fov_degrees: Some(12.0),
            ..camera.clone()
        };
        assert_eq!(zoomed.render_fov_degrees(), 12.0);
        assert_eq!(zoomed.fov_degrees, camera.fov_degrees);
        assert!(PlayerCamera {
            view_fov_degrees: Some(0.0),
            ..camera.clone()
        }
        .validate()
        .is_err());
        assert!(PlayerCamera {
            view_offset: [f32::NAN, 0.0, 0.0],
            ..camera
        }
        .validate()
        .is_err());
        let controls: PlayerControls =
            serde_json::from_value(serde_json::json!({"move_speed": 6.0})).unwrap();
        assert!(controls.bob_enabled && controls.bob_cadence == 2.0);
        assert!(controls.validate().is_ok());
        assert!(PlayerControls {
            bob_vertical: 1.0,
            ..controls
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
