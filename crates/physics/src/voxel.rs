//! Voxelisation of splat centres into occupied cells, the input to rapier's voxel colliders.
//!
//! A cell is occupied when at least one splat centre falls inside it. Indices are relative to `origin`, so a
//! body whose origin is at `origin` can use them directly as its collider's voxel coordinates.

use std::collections::BTreeSet;

/// Occupied cells, sorted and de-duplicated, with the integer cell index `floor((p - origin) / voxel_size)`.
pub fn occupied_cells(points: &[[f32; 3]], voxel_size: f32, origin: [f32; 3]) -> Vec<[i32; 3]> {
    let inverse = 1.0 / voxel_size;
    let mut cells = BTreeSet::new();
    for p in points {
        let cell = [0, 1, 2].map(|i| ((p[i] - origin[i]) * inverse).floor() as i32);
        cells.insert(cell);
    }
    cells.into_iter().collect()
}

/// The centre of mass of the points, used as the body origin so the rigid body rotates about its middle.
pub fn centroid(points: &[[f32; 3]]) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    for p in points {
        for i in 0..3 {
            sum[i] += f64::from(p[i]);
        }
    }
    let n = points.len().max(1) as f64;
    [0, 1, 2].map(|i| (sum[i] / n) as f32)
}

/// Axis-aligned bounds of the cells as `(min_cell, max_cell)`, both inclusive. `None` when empty.
pub fn cell_bounds(cells: &[[i32; 3]]) -> Option<([i32; 3], [i32; 3])> {
    let first = *cells.first()?;
    let mut min = first;
    let mut max = first;
    for c in cells {
        for i in 0..3 {
            min[i] = min[i].min(c[i]);
            max[i] = max[i].max(c[i]);
        }
    }
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_in_the_same_cell_count_once() {
        let points = [[0.01, 0.01, 0.01], [0.02, 0.03, 0.04], [0.11, 0.01, 0.01]];
        let cells = occupied_cells(&points, 0.1, [0.0; 3]);
        assert_eq!(cells, vec![[0, 0, 0], [1, 0, 0]]);
    }

    #[test]
    fn negative_coordinates_floor_towards_minus_infinity() {
        let cells = occupied_cells(&[[-0.01, 0.0, 0.0]], 0.1, [0.0; 3]);
        assert_eq!(cells, vec![[-1, 0, 0]]);
    }

    #[test]
    fn origin_shifts_the_grid() {
        let cells = occupied_cells(&[[1.05, 0.0, 0.0]], 0.1, [1.0, 0.0, 0.0]);
        assert_eq!(cells, vec![[0, 0, 0]]);
    }

    #[test]
    fn centroid_and_bounds() {
        let points = [[0.0, 0.0, 0.0], [2.0, 4.0, 6.0]];
        assert_eq!(centroid(&points), [1.0, 2.0, 3.0]);
        assert_eq!(
            cell_bounds(&[[1, -2, 0], [3, 0, -1]]),
            Some(([1, -2, -1], [3, 0, 0]))
        );
        assert_eq!(cell_bounds(&[]), None);
    }
}
