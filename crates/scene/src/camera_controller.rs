//! Configurable first-person camera behavior, independent of the renderer and scripting host.
use serde::{Deserialize, Serialize};

/// Refers to a stable scene entity ID resolved by the Lua camera controller.
/// Existing `PlayerControls` still owns mouse sensitivity and inversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CameraController {
    pub target: String,
    pub enabled: bool,
    pub pitch_limit_degrees: f32,
    pub turn_tilt_enabled: bool,
    pub turn_tilt_strength: f32,
    pub turn_tilt_max_degrees: f32,
    pub turn_rate_response: f32,
    pub turn_tilt_response: f32,
    pub aim_tilt_scale: f32,
    pub gait_rotation_strength: f32,
    pub aim_bob_scale: f32,
}
impl Default for CameraController {
    fn default() -> Self {
        Self {
            target: "player".into(),
            enabled: true,
            pitch_limit_degrees: 80.0,
            turn_tilt_enabled: true,
            turn_tilt_strength: 1.0,
            turn_tilt_max_degrees: 1.2,
            turn_rate_response: 18.0,
            turn_tilt_response: 12.0,
            aim_tilt_scale: 0.22,
            gait_rotation_strength: 1.0,
            aim_bob_scale: 0.25,
        }
    }
}
impl CameraController {
    pub fn validate(&self) -> Result<(), &'static str> {
        let range = |value: f32, lo: f32, hi: f32| value.is_finite() && (lo..=hi).contains(&value);
        if self.target.is_empty()
            || self.target.len() > 128
            || !self
                .target
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || !range(self.pitch_limit_degrees, 1.0, 89.0)
            || !range(self.turn_tilt_strength, 0.0, 3.0)
            || !range(self.turn_tilt_max_degrees, 0.0, 5.0)
            || !range(self.turn_rate_response, 1.0, 40.0)
            || !range(self.turn_tilt_response, 1.0, 40.0)
            || !range(self.aim_tilt_scale, 0.0, 1.0)
            || !range(self.gait_rotation_strength, 0.0, 3.0)
            || !range(self.aim_bob_scale, 0.0, 1.0)
        {
            return Err("camera controller settings are out of range");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_defaults_and_rejects_invalid_target_and_tuning() {
        let mut c = CameraController::default();
        assert!(c.validate().is_ok());
        c.pitch_limit_degrees = f32::NAN;
        assert!(c.validate().is_err());
        c = CameraController::default();
        c.target = "../player".into();
        assert!(c.validate().is_err());
    }
}
