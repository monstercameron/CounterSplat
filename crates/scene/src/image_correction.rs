//! Scene-owned image correction, independent of any GPU backend.
use serde::{Deserialize, Serialize};

/// Values are applied to linear scene color after the camera exposure and before UI.
/// The highest-priority enabled component wins; ties use the lowest entity ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageCorrection {
    pub enabled: bool,
    pub priority: i32,
    pub filmic: bool,
    pub exposure_ev: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub vignette: f32,
    pub grain: f32,
    /// Edge enhancement applied before color grading (0 disables it).
    pub sharpness: f32,
}
impl Default for ImageCorrection {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 0,
            filmic: true,
            exposure_ev: 0.0,
            contrast: 1.04,
            saturation: 0.96,
            temperature: 0.04,
            tint: 0.0,
            shadows: 0.06,
            highlights: 0.0,
            bloom_intensity: 0.16,
            bloom_threshold: 0.9,
            vignette: 0.12,
            grain: 0.0,
            sharpness: 0.0,
        }
    }
}
impl ImageCorrection {
    pub fn validate(&self) -> Result<(), &'static str> {
        let in_range = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !in_range(self.exposure_ev, -8.0, 8.0)
            || !in_range(self.contrast, 0.25, 2.0)
            || !in_range(self.saturation, 0.0, 2.0)
            || !in_range(self.temperature, -1.0, 1.0)
            || !in_range(self.tint, -1.0, 1.0)
            || !in_range(self.shadows, -1.0, 1.0)
            || !in_range(self.highlights, -1.0, 1.0)
            || !in_range(self.bloom_intensity, 0.0, 2.0)
            || !in_range(self.bloom_threshold, 0.01, 16.0)
            || !in_range(self.vignette, 0.0, 1.0)
            || !in_range(self.grain, 0.0, 0.2)
            || !in_range(self.sharpness, 0.0, 2.0)
        {
            return Err("image correction settings are out of range");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_finite_and_bounded_image_settings() {
        let mut grade = ImageCorrection::default();
        assert!(grade.validate().is_ok());
        grade.bloom_threshold = 0.0;
        assert!(grade.validate().is_err());
        grade.bloom_threshold = 0.9;
        grade.exposure_ev = f32::NAN;
        assert!(grade.validate().is_err());
        grade.exposure_ev = 0.0;
        grade.sharpness = 2.1;
        assert!(grade.validate().is_err());
    }
}
