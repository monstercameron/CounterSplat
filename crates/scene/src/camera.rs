//! Renderer-independent photographic lens settings for any ECS camera.
use serde::{Deserialize, Serialize};

/// Shutter time is angle / (360 × reference FPS). Exposure is relative to the
/// default f/5.6, ISO 100, 180-degree shutter at 60 FPS rather than calibrated luminance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CameraLens {
    pub aperture_f_number: f32,
    pub shutter_angle_degrees: f32,
    pub shutter_fps: f32,
    pub iso: f32,
    pub exposure_compensation_ev: f32,
    pub focus_distance_m: f32,
    pub sensor_height_mm: f32,
    pub depth_of_field: bool,
    pub motion_blur: bool,
}
impl Default for CameraLens {
    fn default() -> Self {
        Self {
            aperture_f_number: 5.6,
            shutter_angle_degrees: 180.0,
            shutter_fps: 60.0,
            iso: 100.0,
            exposure_compensation_ev: 0.0,
            focus_distance_m: 5.0,
            sensor_height_mm: 24.0,
            depth_of_field: false,
            motion_blur: false,
        }
    }
}
impl CameraLens {
    /// Exposure time in seconds, independently of the renderer's actual frame cadence.
    pub fn shutter_seconds(&self) -> f32 {
        self.shutter_angle_degrees / (360.0 * self.shutter_fps)
    }

    /// Relative exposure multiplier, clamped to avoid numerical extremes.
    pub fn exposure_multiplier(&self) -> f32 {
        (self.shutter_seconds()
            * 120.0
            * (5.6 / self.aperture_f_number).powi(2)
            * (self.iso / 100.0)
            * self.exposure_compensation_ev.exp2())
        .clamp(1.0 / 64.0, 64.0)
    }

    /// Pixel diameter coefficient for a thin-lens circle of confusion. `fov_degrees`
    /// is the rendered vertical FOV, so optical zoom changes focal length naturally.
    pub fn blur_scale_pixels(&self, fov_degrees: f32, viewport_height: f32) -> f32 {
        if !self.depth_of_field
            || !fov_degrees.is_finite()
            || !viewport_height.is_finite()
            || !(0.01..179.0).contains(&fov_degrees)
            || viewport_height <= 0.0
        {
            return 0.0;
        }
        let focal_mm = self.sensor_height_mm * 0.5 / (fov_degrees.to_radians() * 0.5).tan();
        let focus_mm = (self.focus_distance_m * 1000.0).max(focal_mm + 1.0);
        (focal_mm * focal_mm / (self.aperture_f_number * (focus_mm - focal_mm)) * viewport_height
            / self.sensor_height_mm)
            .clamp(0.0, 64.0)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.aperture_f_number.is_finite()
            || !(1.0..=32.0).contains(&self.aperture_f_number)
            || !self.shutter_angle_degrees.is_finite()
            || !(1.0..=360.0).contains(&self.shutter_angle_degrees)
            || !self.shutter_fps.is_finite()
            || !(1.0..=240.0).contains(&self.shutter_fps)
            || !self.iso.is_finite()
            || !(25.0..=6400.0).contains(&self.iso)
            || !self.exposure_compensation_ev.is_finite()
            || !(-8.0..=8.0).contains(&self.exposure_compensation_ev)
            || !self.focus_distance_m.is_finite()
            || !(0.1..=1000.0).contains(&self.focus_distance_m)
            || !self.sensor_height_mm.is_finite()
            || !(8.0..=70.0).contains(&self.sensor_height_mm)
        {
            return Err("camera lens settings are out of range");
        }
        Ok(())
    }
}

/// Independently editable photographic settings for the player's zoom lens.
/// Its serialized form uses the same flat fields as CameraLens.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CameraZoomLens(pub CameraLens);
impl CameraZoomLens {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.0.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::CameraLens;
    #[test]
    fn shutter_and_exposure_follow_photographic_ratios() {
        let mut lens = CameraLens::default();
        assert!((lens.shutter_seconds() - 1.0 / 120.0).abs() < 1e-6);
        assert!((lens.exposure_multiplier() - 1.0).abs() < 1e-6);
        lens.shutter_angle_degrees = 90.0;
        assert!((lens.exposure_multiplier() - 0.5).abs() < 1e-6);
        lens.shutter_angle_degrees = 180.0;
        lens.aperture_f_number = 2.8;
        assert!((lens.exposure_multiplier() - 4.0).abs() < 1e-5);
        lens.iso = 200.0;
        lens.exposure_compensation_ev = -1.0;
        assert!((lens.exposure_multiplier() - 4.0).abs() < 1e-5);
    }
    #[test]
    fn focus_blur_depends_on_aperture_and_rejects_bad_settings() {
        let mut lens = CameraLens {
            depth_of_field: true,
            ..Default::default()
        };
        let stopped_down = lens.blur_scale_pixels(60.0, 720.0);
        lens.aperture_f_number = 2.8;
        assert!((lens.blur_scale_pixels(60.0, 720.0) / stopped_down - 2.0).abs() < 1e-4);
        lens.depth_of_field = false;
        assert_eq!(lens.blur_scale_pixels(60.0, 720.0), 0.0);
        lens.aperture_f_number = f32::NAN;
        assert!(lens.validate().is_err());
        lens.aperture_f_number = 5.6;
        lens.shutter_fps = 0.0;
        assert!(lens.validate().is_err());
    }
}
