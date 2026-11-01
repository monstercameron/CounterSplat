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
        ] {
            if key.is_empty() || key.len() > 32 || !key.bytes().all(|c| c.is_ascii_alphanumeric()) {
                return Err("player key bindings must be nonempty physical key codes of at most 32 letters or digits");
            }
        }
        Ok(())
    }
}

/// Script-visible movement settings. Mouse sensitivity is radians per pointer pixel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerControls {
    pub enabled: bool,
    pub mouse_look: bool,
    pub move_speed: f32,
    pub sprint_multiplier: f32,
    pub mouse_sensitivity: f32,
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
            || self
                .fov_degrees
                .is_some_and(|fov| !fov.is_finite() || !(1.0..179.0).contains(&fov))
        {
            return Err(
                "player movement speed, sprint multiplier, or mouse sensitivity is invalid",
            );
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
    fn control_configuration_rejects_invalid_inputs() {
        let mut controls = PlayerControls::default();
        assert!(controls.validate().is_ok());
        controls.mouse_sensitivity = f32::INFINITY;
        assert!(controls.validate().is_err());
        controls.mouse_sensitivity = 0.003;
        controls.bindings.forward = "bad key".into();
        assert!(controls.validate().is_err());
    }
}
