//! Runtime collision against PlayCanvas/SplatTransform sparse voxel octrees.
//!
//! Capsule/sphere-vs-voxel penetration and iterative push-out follow the SuperSplat Viewer
//! approach (PlayCanvas Ltd., MIT); see THIRD_PARTY.md. No dense grid is allocated.
use countersplat_assets::svo::{grid_cells, uses_legacy_coordinates, Octree, LEAF_SIZE};
use std::error::Error;

const EPSILON: f32 = 1.0e-4;
const MAX_ITERATIONS: usize = 4;
const MAX_MOVE_STEPS: usize = 256;
// Bound temporary storage and avoid cache setup when a query visits few rows.
const MAX_CACHED_LEAF_BLOCKS: usize = 256;
const MIN_CACHE_ROWS: u64 = 32;

/// The first occupied voxel reached by a swept world-space segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelRayHit {
    /// Fraction of the original segment, in the inclusive range 0..=1.
    pub fraction: f32,
    pub position: [f32; 3],
    /// Entered voxel face in world space; zero when the segment starts inside solid.
    pub normal: [f32; 3],
}

// Visit only occupied cells in the same z/y/x order as a per-cell scan.
// The caller clips the inclusive bounds to the grid before invoking this helper.
// Reading each leaf once per x row avoids retraversing the octree for all four cells.
fn for_each_solid_cell(tree: &Octree, lo: [i32; 3], hi: [i32; 3], visit: impl FnMut([i32; 3])) {
    // A capsule usually spans only a few dozen leaf blocks per z-slice. Cache
    // that slice as z advances within one leaf to avoid repeated tree walks.
    // Small or very large queries use direct leaf reads with bounded memory.
    let rows = u64::from((hi[1] - lo[1] + 1) as u32) * u64::from((hi[2] - lo[2] + 1) as u32);
    if rows < MIN_CACHE_ROWS {
        return for_each_solid_cell_uncached(tree, lo, hi, visit);
    }
    let x_first = lo[0] as u32 / LEAF_SIZE;
    let x_count = hi[0] as u32 / LEAF_SIZE - x_first + 1;
    let y_first = lo[1] as u32 / LEAF_SIZE;
    let y_count = hi[1] as u32 / LEAF_SIZE - y_first + 1;
    if u64::from(x_count) * u64::from(y_count) > MAX_CACHED_LEAF_BLOCKS as u64 {
        return for_each_solid_cell_uncached(tree, lo, hi, visit);
    }
    for_each_solid_cell_cached(tree, lo, hi, visit, [x_first, x_count, y_first, y_count]);
}

// Keep the cache's stack allocation off the fast path for tiny queries.
#[inline(never)]
fn for_each_solid_cell_cached(
    tree: &Octree,
    lo: [i32; 3],
    hi: [i32; 3],
    mut visit: impl FnMut([i32; 3]),
    [x_first, x_count, y_first, y_count]: [u32; 4],
) {
    let mut cache = [0_u64; MAX_CACHED_LEAF_BLOCKS];
    let mut cached_z = u32::MAX;
    for z in lo[2]..=hi[2] {
        let block_z = z as u32 / LEAF_SIZE;
        if cached_z != block_z {
            for by in 0..y_count {
                for bx in 0..x_count {
                    let index = (by * x_count + bx) as usize;
                    cache[index] = tree.block_mask([
                        (x_first + bx) * LEAF_SIZE,
                        (y_first + by) * LEAF_SIZE,
                        z as u32,
                    ]);
                }
            }
            cached_z = block_z;
        }
        for y in lo[1]..=hi[1] {
            let mut x = lo[0] as u32;
            let hi_x = hi[0] as u32;
            while x <= hi_x {
                let end = (x | 3).min(hi_x);
                let shift = ((y as u32 & 3) * 4) + ((z as u32 & 3) * 16);
                let index = ((y as u32 / LEAF_SIZE - y_first) * x_count + (x / LEAF_SIZE - x_first))
                    as usize;
                let block = cache[index];
                let row = (block >> shift) & 0xf;
                let mut mask = row & (0xf << (x & 3)) & (0xf >> (3 - (end & 3)));
                while mask != 0 {
                    let cell_x = (x & !3) + mask.trailing_zeros();
                    visit([cell_x as i32, y, z]);
                    mask &= mask - 1;
                }
                x = end + 1;
            }
        }
    }
}

fn for_each_solid_cell_uncached(
    tree: &Octree,
    lo: [i32; 3],
    hi: [i32; 3],
    mut visit: impl FnMut([i32; 3]),
) {
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            let mut x = lo[0] as u32;
            let hi_x = hi[0] as u32;
            while x <= hi_x {
                let end = (x | 3).min(hi_x);
                let shift = ((y as u32 & 3) * 4) + ((z as u32 & 3) * 16);
                let row = (tree.block_mask([x, y as u32, z as u32]) >> shift) & 0xf;
                let mut mask = row & (0xf << (x & 3)) & (0xf >> (3 - (end & 3)));
                while mask != 0 {
                    let cell_x = (x & !3) + mask.trailing_zeros();
                    visit([cell_x as i32, y, z]);
                    mask &= mask - 1;
                }
                x = end + 1;
            }
        }
    }
}

pub struct VoxelCollider {
    tree: Octree,
    // Version 1.0 stores X/Y in the old splat coordinate system. Version 1.1 is world space.
    flip_xy: bool,
}
impl VoxelCollider {
    pub fn from_json_and_bin(json: &[u8], bin: &[u8]) -> Result<Self, Box<dyn Error>> {
        let value: serde_json::Value = serde_json::from_slice(json)?;
        let flip_xy = uses_legacy_coordinates(&value)?;
        Ok(Self {
            tree: Octree::from_json_and_bin(json, bin)?,
            flip_xy,
        })
    }
    pub fn resolution(&self) -> f32 {
        self.tree.meta.resolution
    }
    fn local(&self, point: [f32; 3]) -> [f32; 3] {
        if self.flip_xy {
            [-point[0], -point[1], point[2]]
        } else {
            point
        }
    }
    fn world(&self, point: [f32; 3]) -> [f32; 3] {
        self.local(point)
    }

    /// Sweep a point along a finite segment through the sparse voxel tree.
    ///
    /// The grid is clipped first, then a 3-D DDA visits every crossed cell in
    /// order. This avoids tunnelling through thin walls even at projectile speeds.
    /// Both coordinates and the returned face normal use world space.
    pub fn trace_segment(
        &self,
        start: [f32; 3],
        end: [f32; 3],
    ) -> Result<Option<VoxelRayHit>, &'static str> {
        if !start
            .iter()
            .chain(end.iter())
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
        {
            return Err("invalid voxel segment");
        }
        let local_start = self.local(start).map(f64::from);
        let local_end = self.local(end).map(f64::from);
        let delta = [0, 1, 2].map(|i| local_end[i] - local_start[i]);
        let min = self.tree.meta.grid_min.map(f64::from);
        let max = self.tree.meta.grid_max.map(f64::from);
        let resolution = f64::from(self.resolution());
        let dimensions = grid_cells(&self.tree.meta).map(|n| n as i32);
        let mut enter = 0.0_f64;
        let mut exit = 1.0_f64;
        let mut normal = [0.0_f32; 3];
        for axis in 0..3 {
            if delta[axis] == 0.0 {
                if local_start[axis] < min[axis] || local_start[axis] >= max[axis] {
                    return Ok(None);
                }
                continue;
            }
            let a = (min[axis] - local_start[axis]) / delta[axis];
            let b = (max[axis] - local_start[axis]) / delta[axis];
            let near = a.min(b);
            let far = a.max(b);
            if near > enter {
                enter = near;
                normal = [0.0; 3];
                normal[axis] = if delta[axis] > 0.0 { -1.0 } else { 1.0 };
            }
            exit = exit.min(far);
        }
        if enter > exit || exit < 0.0 || enter > 1.0 {
            return Ok(None);
        }
        let mut cell = [0_i32; 3];
        let mut step = [0_i32; 3];
        let mut next = [f64::INFINITY; 3];
        let mut increment = [f64::INFINITY; 3];
        for axis in 0..3 {
            let coordinate = (local_start[axis] + delta[axis] * enter - min[axis]) / resolution;
            let rounded = coordinate.round();
            let boundary = (coordinate - rounded).abs() < 1.0e-8;
            cell[axis] = if boundary && delta[axis] < 0.0 {
                rounded as i32 - 1
            } else {
                coordinate.floor() as i32
            };
            if cell[axis] < 0 || cell[axis] >= dimensions[axis] {
                return Ok(None);
            }
            if delta[axis] != 0.0 {
                step[axis] = if delta[axis] > 0.0 { 1 } else { -1 };
                let boundary_cell = cell[axis] + if step[axis] > 0 { 1 } else { 0 };
                next[axis] = (min[axis] + f64::from(boundary_cell) * resolution
                    - local_start[axis])
                    / delta[axis];
                increment[axis] = resolution / delta[axis].abs();
            }
        }
        let mut t = enter;
        // Queries with unusually tiny voxels fail explicitly rather than skipping collision.
        const MAX_TRACE_CELLS: usize = 1_000_000;
        for _ in 0..MAX_TRACE_CELLS {
            if self.tree.is_solid(cell) {
                let fraction = t.clamp(0.0, 1.0) as f32;
                return Ok(Some(VoxelRayHit {
                    fraction,
                    position: [0, 1, 2].map(|i| start[i] + (end[i] - start[i]) * fraction),
                    normal: self.world(normal),
                }));
            }
            let next_t = next.into_iter().fold(f64::INFINITY, f64::min);
            if next_t > exit || next_t > 1.0 || !next_t.is_finite() {
                return Ok(None);
            }
            normal = [0.0; 3];
            let mut first_axis = true;
            for axis in 0..3 {
                if (next[axis] - next_t).abs() <= 1.0e-10 {
                    cell[axis] += step[axis];
                    next[axis] += increment[axis];
                    if first_axis {
                        normal[axis] = -(step[axis] as f32);
                        first_axis = false;
                    }
                }
            }
            if (0..3).any(|axis| cell[axis] < 0 || cell[axis] >= dimensions[axis]) {
                return Ok(None);
            }
            t = next_t;
        }
        Err("voxel segment exceeds the safe cell budget")
    }

    /// Move a spherical kinematic body with bounded substeps and slide along occupied voxels.
    /// The caller supplies world-space coordinates and a positive radius in level units.
    pub fn move_sphere(
        &self,
        start: [f32; 3],
        delta: [f32; 3],
        radius: f32,
    ) -> Result<[f32; 3], &'static str> {
        self.move_capsule(start, delta, radius, 0.0)
    }

    /// Move an upright capsule against the level octree. `half_height` is half
    /// the length of its vertical center segment, excluding its rounded ends.
    /// Zero reduces to a sphere. Positions and dimensions use level units.
    pub fn move_capsule(
        &self,
        start: [f32; 3],
        delta: [f32; 3],
        radius: f32,
        half_height: f32,
    ) -> Result<[f32; 3], &'static str> {
        if !start
            .iter()
            .chain(delta.iter())
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || !radius.is_finite()
            || !(0.05..=1.0).contains(&radius)
            || !half_height.is_finite()
            || !(0.0..=4.0).contains(&half_height)
        {
            return Err("invalid voxel body position, movement, radius, or capsule height");
        }
        let mut position = self.local(start);
        let delta = self.local(delta);
        let distance = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
        if distance == 0.0 {
            return Ok(start);
        }
        let step = (self.resolution() * 0.5).min(radius * 0.5).max(0.001);
        let steps = (distance / step).ceil().max(1.0) as usize;
        if steps > MAX_MOVE_STEPS {
            return Err("voxel movement exceeds the safe step budget");
        }
        let increment = delta.map(|v| v / steps as f32);
        for _ in 0..steps {
            for axis in 0..3 {
                position[axis] += increment[axis];
            }
            position = self.resolve_capsule(position, radius, half_height);
        }
        Ok(self.world(position))
    }

    /// Test an upright capsule against occupied voxels without moving it.
    /// Touching is allowed; a penetration deeper than the solver tolerance is not.
    pub fn capsule_clear(
        &self,
        center: [f32; 3],
        radius: f32,
        half_height: f32,
    ) -> Result<bool, &'static str> {
        if !center
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || !radius.is_finite()
            || !(0.05..=1.0).contains(&radius)
            || !half_height.is_finite()
            || !(0.0..=4.0).contains(&half_height)
        {
            return Err("invalid voxel capsule clearance query");
        }
        // Limit work on unusually fine grids before the collision owner scans cells.
        let resolution = self.resolution();
        let width = ((2.0 * radius / resolution).ceil() as usize).saturating_add(2);
        let height =
            ((2.0 * (radius + half_height) / resolution).ceil() as usize).saturating_add(2);
        let cells = width.checked_mul(width).and_then(|n| n.checked_mul(height));
        if cells.is_none_or(|count| count > 1_000_000) {
            return Err("voxel clearance query exceeds the safe cell budget");
        }
        Ok(self
            .deepest_capsule_overlap(self.local(center), radius, half_height)
            .is_none())
    }

    /// Find the nearest supported surface below an eye position in world space.
    /// A median of five nearby vertical rays avoids snapping to an isolated voxel.
    /// Returns `None` if fewer than three rays find ground within `max_drop`.
    pub fn ground_height(
        &self,
        eye: [f32; 3],
        sample_radius: f32,
        max_drop: f32,
    ) -> Result<Option<f32>, &'static str> {
        if !eye.iter().all(|v| v.is_finite() && v.abs() <= 10_000_000.0)
            || !sample_radius.is_finite()
            || !(0.0..=1.0).contains(&sample_radius)
            || !max_drop.is_finite()
            || !(0.05..=16.0).contains(&max_drop)
        {
            return Err("invalid ground probe");
        }
        let resolution = self.resolution();
        let steps = (max_drop / resolution).ceil() as usize + 2;
        if steps > 16_384 {
            return Err("ground probe exceeds the step budget");
        }
        let mut hits = Vec::with_capacity(5);
        for (dx, dz) in [
            (0.0, 0.0),
            (sample_radius, 0.0),
            (-sample_radius, 0.0),
            (0.0, sample_radius),
            (0.0, -sample_radius),
        ] {
            let local = self.local([eye[0] + dx, eye[1], eye[2] + dz]);
            let mut cell = [0, 1, 2]
                .map(|i| ((local[i] - self.tree.meta.grid_min[i]) / resolution).floor() as i32);
            // A camera starting inside occupied geometry is not a reliable ground probe.
            if self.tree.is_solid(cell) {
                continue;
            }
            let direction = if self.flip_xy { 1 } else { -1 };
            for _ in 0..steps {
                cell[1] += direction;
                if !self.tree.is_solid(cell) {
                    continue;
                }
                let local_surface = self.tree.meta.grid_min[1]
                    + (cell[1] + if self.flip_xy { 0 } else { 1 }) as f32 * resolution;
                let surface = if self.flip_xy {
                    -local_surface
                } else {
                    local_surface
                };
                if surface <= eye[1] + 0.0001 && eye[1] - surface <= max_drop {
                    hits.push(surface);
                }
                break;
            }
        }
        if hits.len() < 3 {
            return Ok(None);
        }
        hits.sort_by(f32::total_cmp);
        Ok(Some(hits[hits.len() / 2]))
    }

    fn resolve_capsule(&self, mut center: [f32; 3], radius: f32, half_height: f32) -> [f32; 3] {
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(3);
        for _ in 0..MAX_ITERATIONS {
            let Some(push) = self.deepest_capsule_overlap(center, radius, half_height) else {
                break;
            };
            let length = push.iter().map(|v| v * v).sum::<f32>().sqrt();
            let mut constrained = push;
            for normal in &normals {
                let dot = (0..3).map(|i| constrained[i] * normal[i]).sum::<f32>();
                if dot < 0.0 {
                    for i in 0..3 {
                        constrained[i] -= dot * normal[i];
                    }
                }
            }
            if length > EPSILON && normals.len() < 3 {
                normals.push(push.map(|v| v / length));
            }
            for i in 0..3 {
                center[i] += constrained[i];
            }
        }
        center
    }

    fn deepest_capsule_overlap(
        &self,
        center: [f32; 3],
        radius: f32,
        half_height: f32,
    ) -> Option<[f32; 3]> {
        let m = &self.tree.meta;
        let dims = grid_cells(m);
        let extents = [radius, radius + half_height, radius];
        let lo = [0, 1, 2].map(|i| {
            (((center[i] - extents[i] - m.grid_min[i]) / m.resolution).floor() as i32).max(0)
        });
        let hi = [0, 1, 2].map(|i| {
            (((center[i] + extents[i] - m.grid_min[i]) / m.resolution).floor() as i32)
                .min(dims[i] as i32 - 1)
        });
        if (0..3).any(|i| lo[i] > hi[i]) {
            return None;
        }
        let mut best = EPSILON;
        let mut output = None;
        let segment_min = center[1] - half_height;
        let segment_max = center[1] + half_height;
        for_each_solid_cell(&self.tree, lo, hi, |cell| {
            let min = [0, 1, 2].map(|i| m.grid_min[i] + cell[i] as f32 * m.resolution);
            let max = min.map(|v| v + m.resolution);
            // Find the closest pair of points on the vertical segment
            // and voxel box. Their separation gives the capsule normal.
            let segment_y = if segment_max < min[1] {
                segment_max
            } else if segment_min > max[1] {
                segment_min
            } else {
                center[1].clamp(segment_min.max(min[1]), segment_max.min(max[1]))
            };
            let point = [center[0], segment_y, center[2]];
            let near = [0, 1, 2].map(|i| point[i].clamp(min[i], max[i]));
            let offset = [0, 1, 2].map(|i| point[i] - near[i]);
            let distance_sq = offset.iter().map(|v| v * v).sum::<f32>();
            if distance_sq >= radius * radius {
                return;
            }
            let (push, penetration) = if distance_sq > 1.0e-12 {
                let distance = distance_sq.sqrt();
                let penetration = radius - distance;
                (offset.map(|v| v * penetration / distance), penetration)
            } else {
                // The segment intersects the box. Escape on the shortest
                // axis, including the capsule's full vertical extent.
                let escape = [0, 1, 2].map(|i| {
                    let (lower, upper) = if i == 1 {
                        (segment_min, segment_max)
                    } else {
                        (center[i], center[i])
                    };
                    let negative = upper - min[i] + radius;
                    let positive = max[i] - lower + radius;
                    if negative < positive {
                        -negative
                    } else {
                        positive
                    }
                });
                let axis = (0..3)
                    .min_by(|&a, &b| escape[a].abs().total_cmp(&escape[b].abs()))
                    .unwrap();
                let mut push = [0.0; 3];
                push[axis] = escape[axis];
                (push, escape[axis].abs())
            };
            if penetration > best {
                best = penetration;
                output = Some(push);
            }
        });
        output
    }

    /// Sample the closest occupied voxel centers around a world-space point for a bounded debug view.
    pub fn nearby_centers(
        &self,
        center: [f32; 3],
        radius: f32,
        limit: usize,
    ) -> Result<Vec<[f32; 3]>, &'static str> {
        if !center.iter().all(|v| v.is_finite())
            || !radius.is_finite()
            || !(0.1..=8.0).contains(&radius)
            || limit > 10_000
        {
            return Err("invalid voxel debug region");
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let center = self.local(center);
        let m = &self.tree.meta;
        let dims = grid_cells(m);
        let block = LEAF_SIZE as f32 * m.resolution;
        let block_dims = dims.map(|d| d.div_ceil(LEAF_SIZE));
        let lo = [0, 1, 2]
            .map(|i| (((center[i] - radius - m.grid_min[i]) / block).floor() as i32).max(0));
        let hi = [0, 1, 2].map(|i| {
            (((center[i] + radius - m.grid_min[i]) / block).floor() as i32)
                .min(block_dims[i] as i32 - 1)
        });
        if (0..3).any(|i| lo[i] > hi[i]) {
            return Ok(Vec::new());
        }
        let count = (0..3)
            .try_fold(1usize, |n, i| n.checked_mul((hi[i] - lo[i] + 1) as usize))
            .ok_or("voxel debug region too large")?;
        if count > 300_000 {
            return Err("voxel debug region exceeds its block budget");
        }
        let mut blocks = Vec::with_capacity(count);
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let b = [x as u32, y as u32, z as u32];
                    let d = (0..3)
                        .map(|i| (m.grid_min[i] + (b[i] as f32 + 0.5) * block - center[i]).powi(2))
                        .sum::<f32>();
                    blocks.push((d, b));
                }
            }
        }
        blocks.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut out = Vec::with_capacity(limit.min(4096));
        for (_, b) in blocks {
            let origin = b.map(|v| v * LEAF_SIZE);
            let mut mask = self.tree.block_mask(origin);
            while mask != 0 {
                let bit = mask.trailing_zeros();
                mask &= mask - 1;
                let cell = [
                    origin[0] + (bit & 3),
                    origin[1] + ((bit >> 2) & 3),
                    origin[2] + ((bit >> 4) & 3),
                ];
                if (0..3).any(|i| cell[i] >= dims[i]) {
                    continue;
                }
                let point =
                    [0, 1, 2].map(|i| m.grid_min[i] + (cell[i] as f32 + 0.5) * m.resolution);
                if (0..3).map(|i| (point[i] - center[i]).powi(2)).sum::<f32>() <= radius * radius {
                    out.push(self.world(point));
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod occupied_scan_tests {
    use super::*;
    use serde_json::json;

    fn tree(words: &[u32], node_count: usize, leaf_count: usize) -> Octree {
        let metadata = json!({"version":"1.1", "gridBounds":{"min":[0,0,0],"max":[7.3,7.1,6.2]},
            "voxelResolution":1.0, "leafSize":4, "treeDepth":1, "nodeCount":node_count, "leafDataCount":leaf_count});
        let bytes: Vec<_> = words.iter().copied().flat_map(u32::to_le_bytes).collect();
        Octree::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap()
    }

    fn compare_regions(tree: &Octree) {
        let dims = grid_cells(&tree.meta);
        let mut seed = 0x2345_6789_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for sample in 0..4_000 {
            let a = [0, 1, 2].map(|i| (next() % dims[i]) as i32);
            let b = [0, 1, 2].map(|i| (next() % dims[i]) as i32);
            let (lo, hi) = if sample == 0 {
                ([0; 3], dims.map(|n| n as i32 - 1))
            } else {
                (
                    [0, 1, 2].map(|i| a[i].min(b[i])),
                    [0, 1, 2].map(|i| a[i].max(b[i])),
                )
            };
            let mut actual = Vec::new();
            for_each_solid_cell(tree, lo, hi, |cell| actual.push(cell));
            let mut expected = Vec::new();
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        if tree.is_solid([x, y, z]) {
                            expected.push([x, y, z]);
                        }
                    }
                }
            }
            assert_eq!(actual, expected, "bounds {lo:?}..={hi:?}");
        }
    }

    #[test]
    fn leaf_rows_match_per_cell_occupancy_and_order_for_clipped_regions() {
        // Mixed, solid, empty and absent children, including partial grid edges.
        let masks = [
            0x8000_0000_0000_0001_u64,
            0,
            u64::MAX,
            0x5555_aaaa_3333_cccc,
            0x8421_8421_1248_1248,
        ];
        let mut words = vec![(0b1101_1101_u32 << 24) | 1, 0, 1, 0xff00_0000, 2, 3, 4];
        for mask in masks {
            words.extend([mask as u32, (mask >> 32) as u32]);
        }
        compare_regions(&tree(&words, 7, 10));
        // A mixed leaf or solid node may terminate before the declared depth.
        compare_regions(&tree(&[0, 0x8421_8421, 0x1248_1248], 1, 2));
        compare_regions(&tree(&[0xff00_0000], 1, 0));
        compare_regions(&tree(&[0, 0, 0], 1, 2));
    }

    #[test]
    fn cache_at_capacity_preserves_mixed_leaf_order_across_z_slices() {
        let metadata = json!({"version":"1.1", "gridBounds":{"min":[0,0,0],"max":[64,64,8]},
            "voxelResolution":1.0, "leafSize":4, "treeDepth":4, "nodeCount":1, "leafDataCount":2});
        let bytes: Vec<_> = [0_u32, 0x5555_aaaa, 0x8421_1248]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let tree = Octree::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap();
        let lo = [0, 0, 0];
        let hi = [63, 63, 7];
        let mut actual = Vec::new();
        for_each_solid_cell(&tree, lo, hi, |cell| actual.push(cell));
        let mut expected = Vec::new();
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    if tree.is_solid([x, y, z]) {
                        expected.push([x, y, z]);
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn large_regions_use_the_bounded_fallback_without_losing_cells() {
        let metadata = json!({"version":"1.1", "gridBounds":{"min":[0,0,0],"max":[68,68,4]},
            "voxelResolution":1.0, "leafSize":4, "treeDepth":5, "nodeCount":1, "leafDataCount":0});
        let tree = Octree::from_json_and_bin(
            metadata.to_string().as_bytes(),
            &0xff00_0000_u32.to_le_bytes(),
        )
        .unwrap();
        let lo = [0, 0, 1];
        let hi = [67, 67, 2];
        let mut actual = Vec::new();
        for_each_solid_cell(&tree, lo, hi, |cell| actual.push(cell));
        let mut expected = Vec::new();
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    if tree.is_solid([x, y, z]) {
                        expected.push([x, y, z]);
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }
}
