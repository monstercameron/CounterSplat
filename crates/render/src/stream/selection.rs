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

/// Invariant geometry for one forward-biased streaming footprint. Computing
/// it once per query avoids normalizing the same direction for every octree
/// node and leaf; intersection arithmetic stays identical to the reference.
pub(super) struct ForwardEllipseQuery {
    eye_y: f32,
    direction: [f32; 2],
    center: [f32; 2],
    vertical_reach: f32,
    along: f32,
    across: f32,
}

impl ForwardEllipseQuery {
    pub fn new(
        eye: [f32; 3],
        forward: [f32; 3],
        reach: f32,
        rear_fraction: f32,
        side_fraction: f32,
        margin: f32,
    ) -> Self {
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
        Self {
            eye_y: eye[1],
            direction,
            center,
            vertical_reach: reach * margin,
            along: reach * (1.0 + rear) * 0.5 * margin,
            across: reach * side * margin,
        }
    }

    pub fn intersects(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let vertical = (min[1] - self.eye_y).max(0.0).max(self.eye_y - max[1]);
        if vertical > self.vertical_reach {
            return false;
        }
        let center = self.center;
        if center[0] >= min[0] && center[0] <= max[0] && center[1] >= min[2] && center[1] <= max[2]
        {
            return true;
        }
        let direction = self.direction;
        let project = |x: f32, z: f32| {
            let dx = x - center[0];
            let dz = z - center[1];
            [
                (dx * direction[0] + dz * direction[1]) / self.along,
                (-dx * direction[1] + dz * direction[0]) / self.across,
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
}

#[cfg(test)]
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

#[cfg(test)]
mod ellipse_query_tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    #[test]
    fn precomputed_ellipse_matches_previous_intersection_at_boundaries() {
        let mut bits = 0x7135_8badu32;
        let mut next = || {
            bits = bits.wrapping_mul(1664525).wrapping_add(1013904223);
            ((bits >> 8) as f32 / 16_777_215.0 - 0.5) * 400.0
        };
        for i in 0..30_000 {
            let eye = [next(), next(), next()];
            let forward = if i % 17 == 0 {
                [0.0, 1.0, 0.0]
            } else {
                [next(), next(), next()]
            };
            let min = [next(), next(), next()];
            let max = [
                min[0] + next().abs(),
                min[1] + next().abs(),
                min[2] + next().abs(),
            ];
            let reach = [0.0, 0.001, 20.0, 100.0, 500.0][i % 5];
            let rear = [-1.0, 0.05, 0.4, 1.0, 3.0][i % 5];
            let side = [-1.0, 0.05, 0.7, 2.0, 5.0][i % 5];
            let margin = [0.0, 1.0, 1.2, 2.0][i % 4];
            let query = ForwardEllipseQuery::new(eye, forward, reach, rear, side, margin);
            assert_eq!(
                query.intersects(min, max),
                forward_ellipse_intersects(eye, forward, min, max, reach, rear, side, margin),
                "case {i}"
            );
        }
        // Probe exact near/far and side boundaries, including points on both
        // sides where a rounding change could flip a leaf.
        for forward in [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.7, 0.0, -0.3]] {
            for margin in [1.0, 1.2] {
                let eye = [1.0, 2.0, -3.0];
                let query = ForwardEllipseQuery::new(eye, forward, 100.0, 0.3, 0.55, margin);
                for scale in [-1.00001, -1.0, -0.99999, 0.99999, 1.0, 1.00001] {
                    for side in [false, true] {
                        let (dx, dz) = if side {
                            (
                                -query.direction[1] * query.across,
                                query.direction[0] * query.across,
                            )
                        } else {
                            (
                                query.direction[0] * query.along,
                                query.direction[1] * query.along,
                            )
                        };
                        let point = [
                            query.center[0] + dx * scale,
                            eye[1],
                            query.center[1] + dz * scale,
                        ];
                        assert_eq!(
                            query.intersects(point, point),
                            forward_ellipse_intersects(
                                eye, forward, point, point, 100.0, 0.3, 0.55, margin
                            )
                        );
                    }
                }
            }
        }
        for forward in [[0.0; 3], [f32::NAN, 0.0, 1.0], [f32::INFINITY, 0.0, 1.0]] {
            for reach in [0.0, f32::NAN, f32::INFINITY] {
                let eye = [0.0; 3];
                let query = ForwardEllipseQuery::new(eye, forward, reach, 0.3, 0.55, 1.2);
                for (min, max) in [
                    ([0.0; 3], [0.0; 3]),
                    ([-1.0; 3], [1.0; 3]),
                    ([100.0; 3], [101.0; 3]),
                ] {
                    assert_eq!(
                        query.intersects(min, max),
                        forward_ellipse_intersects(eye, forward, min, max, reach, 0.3, 0.55, 1.2)
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_forward_ellipse_query() {
        let boxes: Vec<_> = (0..1024)
            .map(|i| {
                let x = (i % 32) as f32 * 8.0 - 128.0;
                let z = (i / 32) as f32 * 8.0 - 128.0;
                ([x, -1.0, z], [x + 7.0, 4.0, z + 7.0])
            })
            .collect();
        let eye = [1.0, 1.7, -2.0];
        let forward = [0.7, 0.0, -0.3];
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..9 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                let mut hits = 0;
                for _ in 0..200 {
                    let query = ForwardEllipseQuery::new(
                        black_box(eye),
                        black_box(forward),
                        100.0,
                        0.4,
                        0.7,
                        1.2,
                    );
                    for &(min, max) in &boxes {
                        hits += usize::from(if mode == 0 {
                            forward_ellipse_intersects(
                                black_box(eye),
                                black_box(forward),
                                black_box(min),
                                black_box(max),
                                100.0,
                                0.4,
                                0.7,
                                1.2,
                            )
                        } else {
                            query.intersects(black_box(min), black_box(max))
                        });
                    }
                }
                black_box(hits);
                samples[mode]
                    .push(start.elapsed().as_nanos() as f64 / (200.0 * boxes.len() as f64));
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "ellipse per box: previous {:.1} ns, precomputed {:.1} ns",
            samples[0][4], samples[1][4]
        );
    }
}
