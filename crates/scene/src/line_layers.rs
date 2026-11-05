//! World-line layers that independent scene scripts publish (hitboxes, pursuit paths) without clearing one another.
//! The host validates and packs presentation data, not gameplay rules: each segment becomes ten floats (start xyz,
//! finish xyz, colour rgb, alpha) for the line renderer.

use serde_json::Value;
use std::collections::BTreeMap;

/// Most segments drawn across all layers.
pub const MAX_LINE_SEGMENTS: usize = 1024;
const MAX_LAYERS: usize = 8;
const DEFAULT_COLOR: [f32; 3] = [3.2, 1.55, 0.35];
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1), (0, 2), (0, 4), (1, 3), (1, 5), (2, 3), (2, 6), (3, 7), (4, 5), (4, 6), (5, 7), (6, 7),
];

fn vector<const N: usize>(value: Option<&Value>) -> Option<[f64; N]> {
    let array = value?.as_array()?;
    if array.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(array) {
        *slot = item.as_f64().filter(|v| v.is_finite())?;
    }
    Some(out)
}

fn valid_layer(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

#[derive(Debug, Clone, Default)]
pub struct LineLayers {
    layers: BTreeMap<String, Vec<[f32; 10]>>,
    order: Vec<String>,
}

impl LineLayers {
    pub fn clear(&mut self) {
        self.layers.clear();
        self.order.clear();
    }

    /// Replaces one layer from a script snapshot `{layer?, segments?, boxes?}`. False for an invalid layer name or
    /// when a ninth layer would be added; invalid segments and boxes are skipped.
    pub fn update(&mut self, snapshot: &Value) -> bool {
        if !snapshot.is_object() {
            return false;
        }
        let layer = match snapshot.get("layer") {
            None | Some(Value::Null) => "default",
            Some(Value::String(name)) => name.as_str(),
            Some(_) => return false,
        };
        if !valid_layer(layer) || (!self.layers.contains_key(layer) && self.layers.len() >= MAX_LAYERS) {
            return false;
        }
        let mut segments: Vec<[f32; 10]> = Vec::new();
        let lines = snapshot.get("segments").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for line in lines.iter().take(MAX_LINE_SEGMENTS) {
            let (Some(start), Some(finish)) = (vector::<3>(line.get("start")), vector::<3>(line.get("finish"))) else {
                continue;
            };
            let color = match line.get("color") {
                None => Some(DEFAULT_COLOR.map(f64::from)),
                Some(value) => vector::<3>(Some(value)),
            };
            let alpha = line.get("alpha").and_then(Value::as_f64).filter(|a| a.is_finite());
            let (Some(color), Some(alpha)) = (color, alpha) else {
                continue;
            };
            if color.iter().any(|v| !(0.0..=8.0).contains(v))
                || start.iter().chain(&finish).any(|v| v.abs() > 10_000_000.0)
                || !(0.0..=1.0).contains(&alpha)
            {
                continue;
            }
            segments.push(pack(start, finish, color, alpha));
        }
        // Compact oriented boxes become twelve edges each; Lua supplies geometry and colours.
        let boxes = snapshot.get("boxes").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for item in boxes.iter().take(MAX_LINE_SEGMENTS / 12) {
            if segments.len() + 12 > MAX_LINE_SEGMENTS {
                break;
            }
            let (Some(center), Some(half), Some([c, s]), Some(color)) = (
                vector::<3>(item.get("center")),
                vector::<3>(item.get("half_extents")),
                vector::<2>(item.get("rotation")),
                vector::<3>(item.get("color")),
            ) else {
                continue;
            };
            let Some(alpha) = item.get("alpha").and_then(Value::as_f64).filter(|a| (0.0..=1.0).contains(a)) else {
                continue;
            };
            if center.iter().any(|v| v.abs() > 10_000_000.0)
                || half.iter().any(|v| *v <= 0.0 || *v > 10_000.0)
                || color.iter().any(|v| !(0.0..=8.0).contains(v))
                || (c * c + s * s - 1.0).abs() > 0.001
            {
                continue;
            }
            let mut corners = Vec::with_capacity(8);
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let (lx, lz) = (x * half[0], z * half[2]);
                        corners.push([center[0] + c * lx - s * lz, center[1] + y * half[1], center[2] + s * lx + c * lz]);
                    }
                }
            }
            for (a, b) in BOX_EDGES {
                segments.push(pack(corners[a], corners[b], color, alpha));
            }
        }
        if segments.is_empty() {
            self.layers.remove(layer);
            self.order.retain(|name| name != layer);
        } else {
            if !self.layers.contains_key(layer) {
                self.order.push(layer.to_string());
            }
            self.layers.insert(layer.to_string(), segments);
        }
        true
    }

    /// Every layer's segments in the order layers were first published, ten floats each, at most
    /// `MAX_LINE_SEGMENTS`.
    pub fn packed(&self) -> Vec<f32> {
        self.order
            .iter()
            .filter_map(|name| self.layers.get(name))
            .flatten()
            .take(MAX_LINE_SEGMENTS)
            .flatten()
            .copied()
            .collect()
    }
}

fn pack(start: [f64; 3], finish: [f64; 3], color: [f64; 3], alpha: f64) -> [f32; 10] {
    [
        start[0] as f32, start[1] as f32, start[2] as f32,
        finish[0] as f32, finish[1] as f32, finish[2] as f32,
        color[0] as f32, color[1] as f32, color[2] as f32,
        (alpha * 0.9) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layers_are_independent_and_bounded() {
        let mut layers = LineLayers::default();
        let line = json!({"start":[0,1,2],"finish":[3,4,5],"alpha":1});
        assert!(layers.update(&json!({"segments":[line]})));
        assert!(layers.update(&json!({"layer":"pursuit","segments":[{"start":[0,1,2],"finish":[3,4,5],"alpha":1,"color":[3,0.1,0.1]}]})));
        assert_eq!(layers.packed().len(), 20);
        layers.update(&json!({"layer":"pursuit","segments":[]}));
        assert_eq!(layers.packed().len(), 10, "clearing one layer must retain the other");
        layers.update(&json!({"layer":"bad","segments":[{"start":[0,1,2],"finish":[3,4,5],"alpha":1,"color":[-1,0,0]},
            {"start":[null,0,0],"finish":[3,4,5],"alpha":1}]}));
        assert_eq!(layers.packed().len(), 10, "invalid geometry is ignored");
        assert!(!layers.update(&json!({"layer":"invalid name","segments":[line]})));
        let many: Vec<Value> = (0..MAX_LINE_SEGMENTS + 10).map(|_| line.clone()).collect();
        layers.update(&json!({"layer":"full","segments":many}));
        assert_eq!(layers.packed().len(), MAX_LINE_SEGMENTS * 10);
        layers.clear();
        assert!(layers.packed().is_empty());
        let item = json!({"center":[10,20,30],"half_extents":[1,2,3],"rotation":[0,1],"color":[2,0.2,0.1],"alpha":1});
        assert!(layers.update(&json!({"layer":"boxes","boxes":[item]})));
        let packed = layers.packed();
        assert_eq!(packed.len(), 120, "each box has twelve edges");
        assert_eq!(&packed[..6], &[13.0, 18.0, 29.0, 7.0, 18.0, 29.0], "yaw rotation and world position are kept");
        layers.update(&json!({"layer":"boxes","boxes":[
            {"center":[10,20,30],"half_extents":[1,2,3],"rotation":[4,0],"color":[2,0.2,0.1],"alpha":1},
            {"center":[10,20,30],"half_extents":[-1,2,3],"rotation":[0,1],"color":[2,0.2,0.1],"alpha":1}]}));
        assert!(layers.packed().is_empty(), "invalid boxes cannot reach the GPU");
    }
}
