//! Floor heights: cleaning (`fill_holes`, `smooth`), road smoothing (`bilateral`) and merging equal heights into
//! rectangles (`merge`, used for wall columns and the whole-level fallback), one cuboid collider each.
//!
//! The heights come from the level's voxel floor (`LevelFloor`). Rectangles are found greedily: a run along z of
//! equal height, extended along x while every cell in the run matches. Each box is a slab of `SLAB_M` under the
//! walkable top, so dynamic bodies cannot fall through thin collider edges.

/// Thickness of each floor box below its top, in metres.
pub const SLAB_M: f32 = 1.0;

/// A rectangle of floor at one height, in grid cells. `x`, `z` are the low corner; `w` runs along x, `d` along z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorBox {
    pub x: usize,
    pub z: usize,
    pub w: usize,
    pub d: usize,
    /// Top of the floor, in cell units above the grid origin.
    pub top: f32,
}

/// Fills small holes in the floor: a missing cell with at least 4 of its 8 neighbours present takes their median
/// height, repeated `HOLE_PASSES` times so holes up to about twice that many cells across close. Larger gaps (building
/// interiors, the space past the scan's edge) stay open.
pub fn fill_holes(heights: &[Option<f32>], nx: usize, nz: usize) -> Vec<Option<f32>> {
    const HOLE_PASSES: usize = 4;
    let mut current = heights.to_vec();
    for _ in 0..HOLE_PASSES {
        let mut next = current.clone();
        let mut filled = 0;
        for x in 0..nx {
            for z in 0..nz {
                if current[x * nz + z].is_some() {
                    continue;
                }
                let mut around = neighbours(&current, nx, nz, x, z, 1);
                if around.len() >= 4 {
                    next[x * nz + z] = Some(median(&mut around));
                    filled += 1;
                }
            }
        }
        current = next;
        if filled == 0 {
            break;
        }
    }
    current
}

/// A 5 × 5 median over the floor heights. The voxel floor is quantised to its cell size and noisy (cobbles, scan
/// speckle); the median keeps kerbs and steps sharp while removing single-cell spikes and pits that would kick the
/// moving bodies. Holes stay holes.
pub fn smooth(heights: &[Option<f32>], nx: usize, nz: usize) -> Vec<Option<f32>> {
    let mut out = heights.to_vec();
    for x in 0..nx {
        for z in 0..nz {
            if heights[x * nz + z].is_none() {
                continue;
            }
            let mut around = neighbours(heights, nx, nz, x, z, 2);
            out[x * nz + z] = Some(median(&mut around));
        }
    }
    out
}

/// Edge-preserving smoothing of the road surface (a separable bilateral filter). Each height becomes the Gaussian
/// average of its neighbours within `radius` cells, but a neighbour counts less the further its height is from the
/// cell's and not at all beyond `keep_step`. Cobbles, grass tufts, scan noise and the voxels' height steps average out
/// into a smooth surface; kerbs, steps and ledges taller than `keep_step` keep their edge, as neither side averages
/// in the other. Holes stay holes and are never averaged in.
pub fn bilateral(
    heights: &[Option<f32>],
    nx: usize,
    nz: usize,
    radius: usize,
    keep_step: f32,
) -> Vec<Option<f32>> {
    if radius == 0 || keep_step <= 0.0 {
        return heights.to_vec();
    }
    let sigma = radius as f32 / 2.0;
    let spatial: Vec<f32> = (0..=radius)
        .map(|d| (-((d * d) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let pass = |src: &[Option<f32>], along_x: bool| -> Vec<Option<f32>> {
        let mut out = src.to_vec();
        for x in 0..nx {
            for z in 0..nz {
                let Some(centre) = src[x * nz + z] else {
                    continue;
                };
                let (mut sum, mut total) = (0.0, 0.0);
                let (pos, len) = if along_x { (x, nx) } else { (z, nz) };
                for k in pos.saturating_sub(radius)..(pos + radius + 1).min(len) {
                    let index = if along_x { k * nz + z } else { x * nz + k };
                    let Some(h) = src[index] else { continue };
                    let range = 1.0 - ((h - centre) / keep_step).powi(2);
                    if range <= 0.0 {
                        continue;
                    }
                    let w = spatial[pos.abs_diff(k)] * range * range;
                    sum += h * w;
                    total += w;
                }
                out[x * nz + z] = Some(sum / total);
            }
        }
        out
    };
    pass(&pass(heights, true), false)
}

/// The present heights within `r` cells of (x, z), including the cell itself.
fn neighbours(
    heights: &[Option<f32>],
    nx: usize,
    nz: usize,
    x: usize,
    z: usize,
    r: usize,
) -> Vec<f32> {
    let mut values = Vec::with_capacity((2 * r + 1) * (2 * r + 1));
    for i in x.saturating_sub(r)..(x + r + 1).min(nx) {
        for j in z.saturating_sub(r)..(z + r + 1).min(nz) {
            if let Some(h) = heights[i * nz + j] {
                values.push(h);
            }
        }
    }
    values
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Greedy rectangles over an `nx × nz` height grid (row-major by x). `None` cells are holes and produce no box.
pub fn merge(heights: &[Option<f32>], nx: usize, nz: usize) -> Vec<FloorBox> {
    let mut visited = vec![false; nx * nz];
    let mut boxes = Vec::new();
    for x in 0..nx {
        for z in 0..nz {
            let index = x * nz + z;
            if visited[index] {
                continue;
            }
            let Some(top) = heights[index] else { continue };
            // Extend along z while the height matches.
            let mut d = 0;
            while z + d < nz && !visited[x * nz + z + d] && heights[x * nz + z + d] == Some(top) {
                d += 1;
            }
            // Extend along x while the whole run of `d` cells matches.
            let mut w = 1;
            'grow: while x + w < nx {
                for k in 0..d {
                    let i = (x + w) * nz + z + k;
                    if visited[i] || heights[i] != Some(top) {
                        break 'grow;
                    }
                }
                w += 1;
            }
            for dx in 0..w {
                for dz in 0..d {
                    visited[(x + dx) * nz + z + dz] = true;
                }
            }
            boxes.push(FloorBox { x, z, w, d, top });
        }
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilateral_smooths_cobbles_but_keeps_a_kerb() {
        // 40 cells along x: a road with ±3 cm cobbles, then a 15 cm kerb from x = 20.
        let (nx, nz) = (40, 3);
        let heights: Vec<Option<f32>> = (0..nx * nz)
            .map(|i| {
                let x = i / nz;
                let cobble = if (x + i % nz) % 2 == 0 { 0.03 } else { -0.03 };
                Some(cobble + if x >= 20 { 0.15 } else { 0.0 })
            })
            .collect();
        let out = bilateral(&heights, nx, nz, 5, 0.08);
        let at = |x: usize| out[x * nz + 1].unwrap();
        for x in (2..17).chain(23..38) {
            let road = if x >= 20 { 0.15 } else { 0.0 };
            assert!(
                (at(x) - road).abs() < 0.012,
                "cell {x} smoothed to {} (road at {road})",
                at(x)
            );
        }
        assert!(
            at(21) - at(18) > 0.12,
            "the kerb keeps its height: {} → {}",
            at(18),
            at(21)
        );
        // Holes stay holes.
        let mut holed = heights.clone();
        holed[5 * nz + 1] = None;
        assert_eq!(bilateral(&holed, nx, nz, 5, 0.08)[5 * nz + 1], None);
    }

    #[test]
    fn a_flat_floor_is_one_box() {
        let heights = vec![Some(2.0); 4 * 3];
        let boxes = merge(&heights, 4, 3);
        assert_eq!(
            boxes,
            vec![FloorBox {
                x: 0,
                z: 0,
                w: 4,
                d: 3,
                top: 2.0
            }]
        );
    }

    #[test]
    fn a_step_splits_into_two_boxes_and_holes_are_skipped() {
        // Two columns of height 1 then a hole, then height 3.
        let heights = vec![Some(1.0), Some(1.0), None, Some(3.0), Some(1.0), None];
        let boxes = merge(&heights, 3, 2);
        let covered: usize = boxes.iter().map(|b| b.w * b.d).sum();
        assert_eq!(covered, 4, "every non-hole cell is covered exactly once");
        assert!(boxes.iter().all(|b| b.top == 1.0 || b.top == 3.0));
    }

    #[test]
    fn every_cell_is_covered_exactly_once() {
        let heights: Vec<Option<f32>> = (0..(7 * 5))
            .map(|i| {
                if i % 4 == 0 {
                    None
                } else {
                    Some((i % 3) as f32)
                }
            })
            .collect();
        let boxes = merge(&heights, 7, 5);
        let mut seen = vec![0u32; 7 * 5];
        for b in &boxes {
            for x in b.x..b.x + b.w {
                for z in b.z..b.z + b.d {
                    seen[x * 5 + z] += 1;
                    assert_eq!(heights[x * 5 + z], Some(b.top));
                }
            }
        }
        for (i, h) in heights.iter().enumerate() {
            assert_eq!(seen[i], if h.is_some() { 1 } else { 0 }, "cell {i}");
        }
    }
}
