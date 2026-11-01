//! Pure screen-space LOD, distance, and frustum helpers.
use super::StreamSettings;
use crate::lod::Range;

/// Evaluate a level using the same settings in selection and prefetch, without borrowing the whole stream.
pub(super) fn level_at_values(
    spacing: &[Vec<f32>],
    settings: &StreamSettings,
    detail_scale: f32,
    focal_px: f32,
    leaf: usize,
    distance: f32,
) -> f32 {
    let error_px = settings.lod_error_px;
    if error_px <= 0.0 {
        let d0 = (settings.lod_distance_m * detail_scale).max(0.1);
        return if distance <= d0 {
            0.0
        } else {
            (distance / d0).log2() + 1.0
        };
    }
    // A smaller budget allows a larger error.
    let target = error_px / detail_scale;
    let error = |l: usize| spacing[leaf][l] * focal_px / distance.max(0.05);
    let levels = spacing[leaf].len();
    if error(0) >= target {
        return 0.0;
    }
    for l in 0..levels.saturating_sub(1) {
        let (a, b) = (error(l), error(l + 1));
        if b > target {
            // Between l (fine enough) and l + 1 (too coarse): how far in log space.
            return l as f32 + ((target / a).ln() / (b / a).max(1.0001).ln()).clamp(0.0, 0.999);
        }
    }
    (levels - 1) as f32
}

pub(super) fn leaf_count(leaf: &crate::lod::Leaf, lod: usize) -> i64 {
    i64::from(leaf.lods[lod].map_or(0, |range| range.count))
}

/// How far a leaf's splats are from `p`, for choosing its detail: the distance to the nearest point of its box. A
/// long leaf (a facade down a street) is seen up close at its near end, so that end decides its detail; measuring to
/// its middle instead showed the near end of long walls coarse.
pub(super) fn effective_distance(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    box_distance(p, min, max)
}

/// Distance from `p` to the box [min, max] (0 inside).
fn box_distance(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    (0..3)
        .map(|i| (min[i] - p[i]).max(0.0).max(p[i] - max[i]).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// The view frustum's side planes, widened to `margin` half-screens, and its front (the plane through the camera),
/// as (a, b, c, d) with a·x + b·y + c·z + d ≥ 0 inside, from a column-major clip matrix.
pub(super) fn frustum_planes(clip: &[f32; 16], margin: f32) -> [[f32; 4]; 5] {
    let row = |r: usize| [clip[r], clip[4 + r], clip[8 + r], clip[12 + r]];
    let (x, y, w) = (row(0), row(1), row(3));
    let plane = |s: f32, a: [f32; 4]| [0, 1, 2, 3].map(|k| margin * w[k] + s * a[k]);
    [
        plane(1.0, x),
        plane(-1.0, x),
        plane(1.0, y),
        plane(-1.0, y),
        w,
    ]
}

/// Whether the box [min, max] reaches inside every plane (it may still lie outside near a corner, which is fine).
pub(super) fn box_in_frustum(planes: &[[f32; 4]; 5], min: [f32; 3], max: [f32; 3]) -> bool {
    planes.iter().all(|p| {
        // The box corner farthest along the plane's normal.
        let c = [0, 1, 2].map(|i| if p[i] >= 0.0 { max[i] } else { min[i] });
        p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] >= 0.0
    })
}

pub(super) fn coarsest_available(lods: &[Option<Range>]) -> Option<usize> {
    (0..lods.len()).rev().find(|&l| lods[l].is_some())
}

/// Conservative intersection between a level leaf and a horizontal, forward-biased oval.
/// The footprint is an ellipse extending `reach` ahead and `rear_fraction * reach` behind.
/// Transforming its rectangle into ellipse space makes edge intersection exact in the ground plane.
pub(super) fn forward_ellipse_intersects(
    eye: [f32; 3],
    forward: [f32; 3],
    min: [f32; 3],
    max: [f32; 3],
    reach: f32,
    rear_fraction: f32,
    side_fraction: f32,
    margin: f32,
) -> bool {
    let reach = reach.max(0.001);
    let rear = rear_fraction.clamp(0.05, 1.0);
    let side = side_fraction.clamp(0.05, 2.0);
    let margin = margin.max(0.001);
    let horizontal = (forward[0] * forward[0] + forward[2] * forward[2]).sqrt();
    let direction = if horizontal > 0.00001 {
        [forward[0] / horizontal, forward[2] / horizontal]
    } else {
        [0.0, -1.0]
    };
    let center = [
        eye[0] + direction[0] * reach * (1.0 - rear) * 0.5,
        eye[2] + direction[1] * reach * (1.0 - rear) * 0.5,
    ];
    let vertical = (min[1] - eye[1]).max(0.0).max(eye[1] - max[1]);
    if vertical > reach * margin {
        return false;
    }
    if center[0] >= min[0] && center[0] <= max[0] && center[1] >= min[2] && center[1] <= max[2] {
        return true;
    }
    let along = reach * (1.0 + rear) * 0.5 * margin;
    let across = reach * side * margin;
    let project = |x: f32, z: f32| {
        let dx = x - center[0];
        let dz = z - center[1];
        [
            (dx * direction[0] + dz * direction[1]) / along,
            (-dx * direction[1] + dz * direction[0]) / across,
        ]
    };
    let corners = [
        project(min[0], min[2]),
        project(max[0], min[2]),
        project(max[0], max[2]),
        project(min[0], max[2]),
    ];
    (0..4).any(|i| {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let edge = [b[0] - a[0], b[1] - a[1]];
        let length_sq = edge[0] * edge[0] + edge[1] * edge[1];
        let t = if length_sq > 0.0 {
            (-(a[0] * edge[0] + a[1] * edge[1]) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let closest = [a[0] + t * edge[0], a[1] + t * edge[1]];
        closest[0] * closest[0] + closest[1] * closest[1] <= 1.0
    })
}
