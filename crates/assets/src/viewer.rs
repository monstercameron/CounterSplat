//! Optional initial camera hints from SuperSplat `viewer-settings.json` files.

/// Initial camera in the coordinate system recorded by the source viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraHint {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub fov_degrees: f32,
}

impl CameraHint {
    /// Parses the first initial camera. Malformed or degenerate camera data is reported to the caller.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let document: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let camera = &document["cameras"][0]["initial"];
        let vector = |value: &serde_json::Value| -> Option<[f32; 3]> {
            let values = value.as_array().filter(|values| values.len() == 3)?;
            let result = [
                values[0].as_f64()? as f32,
                values[1].as_f64()? as f32,
                values[2].as_f64()? as f32,
            ];
            result
                .iter()
                .all(|value| value.is_finite())
                .then_some(result)
        };
        let position = vector(&camera["position"]).ok_or("camera position missing or invalid")?;
        let target = vector(&camera["target"]).ok_or("camera target missing or invalid")?;
        let fov = camera["fov"].as_f64().unwrap_or(60.0) as f32;
        if !fov.is_finite()
            || !(1.0..179.0).contains(&fov)
            || (0..3)
                .map(|i| (target[i] - position[i]).powi(2))
                .sum::<f32>()
                < 1e-8
        {
            return Err("camera field of view or direction is invalid".into());
        }
        Ok(Self {
            position,
            target,
            fov_degrees: fov,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_validates_camera_hints() {
        let json = br#"{"cameras":[{"initial":{"position":[1,2,3],"target":[0,1,0],"fov":90}}]}"#;
        let hint = CameraHint::from_json(json).unwrap();
        assert_eq!(hint.position, [1.0, 2.0, 3.0]);
        assert_eq!(hint.fov_degrees, 90.0);
        assert!(CameraHint::from_json(
            br#"{"cameras":[{"initial":{"position":[0,0,0],"target":[0,0,0]}}]}"#
        )
        .is_err());
    }
}
