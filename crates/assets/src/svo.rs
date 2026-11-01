//! Reader for SuperSplat / SplatTransform collision voxels (`scene.voxel.json` + `scene.voxel.bin`, version 1.x).
//!
//! The format is a sparse voxel octree at `voxelResolution` metres per cell over `gridBounds`. Each node is one
//! `u32` word:
//! - `0xff000000`: the whole node is solid.
//! - Interior node: the top byte is the child mask (bit = octant, x is bit 0). The low 24 bits index the first child,
//!   and children are stored contiguously in popcount order.
//! - Leaf (at `treeDepth`): the low 24 bits index a *mixed* leaf. Each mixed leaf is two words: a 64-bit mask of a
//!   4×4×4 block (bit = x + 4y + 16z). Mixed-leaf words live after all node words.
//!
//! The binary decoder is independent of physics and rendering.

use std::fs;
use std::path::Path;

const SOLID: u32 = 0xff00_0000;
const INDEX_MASK: u32 = 0x00ff_ffff;
/// Cells per leaf edge.
pub const LEAF_SIZE: u32 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub grid_min: [f32; 3],
    pub grid_max: [f32; 3],
    pub resolution: f32,
    pub tree_depth: u32,
    pub node_count: usize,
    pub leaf_data_count: usize,
}

#[derive(Debug)]
pub struct Octree {
    pub meta: Meta,
    words: Vec<u32>,
}

/// Grid dimensions in cells: `ceil((max - min) / resolution)` per axis.
pub fn grid_cells(meta: &Meta) -> [u32; 3] {
    [0, 1, 2].map(|i| ((meta.grid_max[i] - meta.grid_min[i]) / meta.resolution).ceil() as u32)
}

/// Tight obstacle box, in metres relative to the collision grid origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Octree {
    /// Loads `scene.voxel.json` and `scene.voxel.bin` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_json_and_bin(
            &fs::read(dir.join("scene.voxel.json"))?,
            &fs::read(dir.join("scene.voxel.bin"))?,
        )
    }

    /// Builds the octree from the two files' contents, for callers without a file system (the browser build).
    pub fn from_json_and_bin(json: &[u8], bin: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let meta = parse_meta(&serde_json::from_slice(json)?)?;
        Self::from_bytes(meta, bin)
    }

    /// Builds the octree from metadata and the raw little-endian `.bin` contents, checking the counts first.
    pub fn from_bytes(meta: Meta, bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let expected = meta.node_count + meta.leaf_data_count;
        if bytes.len() != expected * 4 {
            return Err(format!(
                "voxel binary is {} bytes; expected {} words",
                bytes.len(),
                expected
            )
            .into());
        }
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Ok(Self { meta, words })
    }

    /// Calls `visit` with the index of every occupied cell, relative to `grid_min`, in x-y-z cell units.
    /// Each mixed leaf contributes its occupied cells; solid nodes contribute every cell they cover.
    pub fn for_each_occupied(&self, mut visit: impl FnMut([u32; 3])) {
        let dims = grid_cells(&self.meta);
        // (node index, depth, origin in cells)
        let mut stack: Vec<(usize, u32, [u32; 3])> = vec![(0, 0, [0, 0, 0])];
        while let Some((node, depth, origin)) = stack.pop() {
            let word = self.words[node];
            let span = LEAF_SIZE << (self.meta.tree_depth - depth);
            if word == SOLID {
                for z in origin[2]..(origin[2] + span).min(dims[2]) {
                    for y in origin[1]..(origin[1] + span).min(dims[1]) {
                        for x in origin[0]..(origin[0] + span).min(dims[0]) {
                            visit([x, y, z]);
                        }
                    }
                }
                continue;
            }
            if depth >= self.meta.tree_depth {
                let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
                for bit in 0..64u32 {
                    if self.words[base + (bit / 32) as usize] & (1 << (bit % 32)) != 0 {
                        let cell = [
                            origin[0] + (bit & 3),
                            origin[1] + ((bit >> 2) & 3),
                            origin[2] + ((bit >> 4) & 3),
                        ];
                        if cell[0] < dims[0] && cell[1] < dims[1] && cell[2] < dims[2] {
                            visit(cell);
                        }
                    }
                }
                continue;
            }
            let mask = word >> 24;
            let first = (word & INDEX_MASK) as usize;
            let child_span = LEAF_SIZE << (self.meta.tree_depth - depth - 1);
            let mut offset = 0usize;
            for octant in 0..8u32 {
                if mask & (1 << octant) != 0 {
                    let child_origin = [
                        origin[0] + if octant & 1 != 0 { child_span } else { 0 },
                        origin[1] + if octant & 2 != 0 { child_span } else { 0 },
                        origin[2] + if octant & 4 != 0 { child_span } else { 0 },
                    ];
                    stack.push((first + offset, depth + 1, child_origin));
                    offset += 1;
                }
            }
        }
    }

    /// Per-block state of the whole grid, `0` empty, `1` solid, `2` mixed, indexed `(bz * nbx + bx) * nby + by`.
    /// A block is one leaf (4×4×4 cells). Solid nodes fill every block they cover, clipped to the grid.
    fn block_states(&self) -> (Vec<u8>, [usize; 3]) {
        let dims = grid_cells(&self.meta);
        let bdims = [0, 1, 2].map(|i| (dims[i] as usize).div_ceil(LEAF_SIZE as usize));
        let mut state = vec![0u8; bdims[0] * bdims[1] * bdims[2]];
        let index = |b: [usize; 3]| (b[2] * bdims[0] + b[0]) * bdims[1] + b[1];
        let mut stack: Vec<(usize, u32, [u32; 3])> = vec![(0, 0, [0, 0, 0])];
        while let Some((node, depth, origin)) = stack.pop() {
            let span = LEAF_SIZE << (self.meta.tree_depth - depth);
            if (0..3).any(|i| origin[i] >= dims[i]) {
                continue;
            }
            let word = self.words[node];
            if word == SOLID || depth >= self.meta.tree_depth {
                let value = if word == SOLID {
                    1
                } else {
                    let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
                    let mask = self.words[base] as u64 | ((self.words[base + 1] as u64) << 32);
                    if mask == 0 {
                        continue;
                    }
                    2
                };
                let (lo, hi) = (
                    origin.map(|v| v as usize / LEAF_SIZE as usize),
                    [0, 1, 2].map(|i| {
                        ((origin[i] + span).min(dims[i]) as usize).div_ceil(LEAF_SIZE as usize)
                    }),
                );
                for bz in lo[2]..hi[2] {
                    for bx in lo[0]..hi[0] {
                        for by in lo[1]..hi[1] {
                            state[index([bx, by, bz])] = value;
                        }
                    }
                }
                continue;
            }
            let mask = word >> 24;
            let first = (word & INDEX_MASK) as usize;
            let child_span = LEAF_SIZE << (self.meta.tree_depth - depth - 1);
            let mut offset = 0usize;
            for octant in 0..8u32 {
                if mask & (1 << octant) != 0 {
                    let child_origin = [
                        origin[0] + if octant & 1 != 0 { child_span } else { 0 },
                        origin[1] + if octant & 2 != 0 { child_span } else { 0 },
                        origin[2] + if octant & 4 != 0 { child_span } else { 0 },
                    ];
                    stack.push((first + offset, depth + 1, child_origin));
                    offset += 1;
                }
            }
        }
        (state, bdims)
    }

    /// Occupancy mask (bit = x + 4y + 16z) of the leaf block that contains `cell`, or `0` when empty.
    fn leaf_mask(&self, cell: [u32; 3]) -> u64 {
        let mut node = 0usize;
        for depth in 0..self.meta.tree_depth {
            let word = self.words[node];
            if word == SOLID {
                return u64::MAX;
            }
            let mask = word >> 24;
            let shift = self.meta.tree_depth - depth - 1 + 2;
            let octant = ((cell[0] >> shift) & 1)
                | (((cell[1] >> shift) & 1) << 1)
                | (((cell[2] >> shift) & 1) << 2);
            if mask & (1 << octant) == 0 {
                return 0;
            }
            node =
                (word & INDEX_MASK) as usize + (mask & ((1 << octant) - 1)).count_ones() as usize;
        }
        let word = self.words[node];
        if word == SOLID {
            return u64::MAX;
        }
        let base = self.meta.node_count + (word & INDEX_MASK) as usize * 2;
        self.words[base] as u64 | ((self.words[base + 1] as u64) << 32)
    }

    /// Walls, per block column (4×4 cells): `Some(r)` where something solid stands within `band` blocks above the
    /// local road, whose top block is `r`. The road under a column is the lowest floor (see `floor_cells`) within
    /// `reach` blocks, so a facade or a post next to the street finds the street's height. Columns with open air above
    /// their own floor are never walls. Returns the grid, indexed `bx * nbz + bz`, and its size `[nbx, nbz]`.
    pub fn wall_columns(
        &self,
        clearance: u32,
        band: (usize, usize),
        reach: usize,
    ) -> (Vec<Option<u32>>, [usize; 2]) {
        let (state, bdims) = self.block_states();
        let clear_blocks = clearance.div_ceil(LEAF_SIZE) as usize;
        let (nbx, nby, nbz) = (bdims[0], bdims[1], bdims[2]);
        let column = |bx: usize, bz: usize| {
            let start = (bz * nbx + bx) * nby;
            &state[start..start + nby]
        };
        // The floor block of every column, as `floor_cells` finds it.
        let mut ground = vec![None::<usize>; nbx * nbz];
        for bz in 0..nbz {
            for bx in 0..nbx {
                let col = column(bx, bz);
                ground[bx * nbz + bz] = (0..nby).find(|&by| {
                    col[by] != 0
                        && (by + 1..nby).take_while(|&j| col[j] == 0).count() >= clear_blocks.max(1)
                });
            }
        }
        let reach = reach as isize;
        let mut walls = vec![None; nbx * nbz];
        for bz in 0..nbz {
            for bx in 0..nbx {
                let mut road = None::<usize>;
                for dx in -reach..=reach {
                    for dz in -reach..=reach {
                        let (x, z) = (bx as isize + dx, bz as isize + dz);
                        if x < 0 || z < 0 || x >= nbx as isize || z >= nbz as isize {
                            continue;
                        }
                        if let Some(g) = ground[x as usize * nbz + z as usize] {
                            road = Some(road.map_or(g, |r| r.min(g)));
                        }
                    }
                }
                let Some(road) = road else { continue };
                let col = column(bx, bz);
                let (lo, hi) = ((road + band.0).min(nby), (road + band.1).min(nby));
                if col[lo..hi].iter().any(|&b| b != 0) {
                    walls[bx * nbz + bz] = Some(road as u32);
                }
            }
        }
        (walls, [nbx, nbz])
    }

    /// Refine candidate wall blocks using their actual voxel masks. Empty parts of mixed
    /// leaves and vertical gaps stay empty; isolated scan voxels do not become barriers.
    /// Greedy boxes are bounded by occupied cells, never extruded to an invented height.
    pub fn wall_boxes(
        &self,
        walls: &[Option<u32>],
        dims: [usize; 2],
        band: (usize, usize),
    ) -> Vec<WallBox> {
        let grid = grid_cells(&self.meta);
        let nby = (grid[1] as usize).div_ceil(4);
        let mut boxes = Vec::new();
        let res = self.meta.resolution;
        for bx in 0..dims[0] {
            for bz in 0..dims[1] {
                let Some(road) = walls[bx * dims[1] + bz] else {
                    continue;
                };
                let lo = (road as usize + band.0).min(nby);
                let hi = (road as usize + band.1).min(nby);
                if lo >= hi {
                    continue;
                }
                let ny = (hi - lo) * 4;
                let index = |x: usize, y: usize, z: usize| (x * 4 + z) * ny + y;
                let mut occupied = vec![false; 16 * ny];
                for by in lo..hi {
                    let mut mask =
                        self.leaf_mask([(bx * 4) as u32, (by * 4) as u32, (bz * 4) as u32]);
                    while mask != 0 {
                        let bit = mask.trailing_zeros() as usize;
                        let (x, y, z) = (bit & 3, (bit >> 2) & 3, (bit >> 4) & 3);
                        if bx * 4 + x < grid[0] as usize
                            && by * 4 + y < grid[1] as usize
                            && bz * 4 + z < grid[2] as usize
                        {
                            occupied[index(x, (by - lo) * 4 + y, z)] = true;
                        }
                        mask &= mask - 1;
                    }
                }
                // Require local surface/line support. A real thin wall, post or sheet has
                // at least two face neighbours; one stray occupied voxel does not.
                let raw = occupied.clone();
                for x in 0..4 {
                    for z in 0..4 {
                        for y in 0..ny {
                            let k = index(x, y, z);
                            if !raw[k] {
                                continue;
                            }
                            let neighbours = [
                                (x.checked_sub(1), Some(y), Some(z)),
                                ((x + 1 < 4).then_some(x + 1), Some(y), Some(z)),
                                (Some(x), y.checked_sub(1), Some(z)),
                                (Some(x), (y + 1 < ny).then_some(y + 1), Some(z)),
                                (Some(x), Some(y), z.checked_sub(1)),
                                (Some(x), Some(y), (z + 1 < 4).then_some(z + 1)),
                            ]
                            .iter()
                            .filter(|(a, b, c)| match (a, b, c) {
                                (Some(a), Some(b), Some(c)) => raw[index(*a, *b, *c)],
                                _ => false,
                            })
                            .count();
                            if neighbours < 2 {
                                occupied[k] = false;
                            }
                        }
                    }
                }
                for y in 0..ny {
                    for x in 0..4 {
                        for z in 0..4 {
                            if !occupied[index(x, y, z)] {
                                continue;
                            }
                            let mut d = 1;
                            while z + d < 4 && occupied[index(x, y, z + d)] {
                                d += 1;
                            }
                            let mut w = 1;
                            while x + w < 4 && (z..z + d).all(|zz| occupied[index(x + w, y, zz)]) {
                                w += 1;
                            }
                            let mut h = 1;
                            while y + h < ny
                                && (x..x + w)
                                    .all(|xx| (z..z + d).all(|zz| occupied[index(xx, y + h, zz)]))
                            {
                                h += 1;
                            }
                            for xx in x..x + w {
                                for zz in z..z + d {
                                    for yy in y..y + h {
                                        occupied[index(xx, yy, zz)] = false;
                                    }
                                }
                            }
                            boxes.push(WallBox {
                                min: [
                                    (bx * 4 + x) as f32 * res,
                                    (lo * 4 + y) as f32 * res,
                                    (bz * 4 + z) as f32 * res,
                                ],
                                max: [
                                    (bx * 4 + x + w) as f32 * res,
                                    (lo * 4 + y + h) as f32 * res,
                                    (bz * 4 + z + d) as f32 * res,
                                ],
                            });
                        }
                    }
                }
            }
        }
        boxes
    }

    /// Floor cells: for each 4×4 column of cells, the lowest solid-to-empty surface that has at least `clearance`
    /// cells of air above it (or the top of the grid). Returns cell indices `[x, y, z]` of the solid cells whose top
    /// face is the walkable surface, one per (x, z) column in that footprint.
    ///
    /// Works per block so memory stays at one byte per 64 cells, not per occupied cell.
    pub fn floor_cells(&self, clearance: u32) -> Vec<[u32; 3]> {
        let dims = grid_cells(&self.meta);
        let (state, bdims) = self.block_states();
        let clear_blocks = clearance.div_ceil(LEAF_SIZE) as usize;
        let index = |bx: usize, by: usize, bz: usize| (bz * bdims[0] + bx) * bdims[1] + by;
        let mut floor = Vec::new();
        for bz in 0..bdims[2] {
            for bx in 0..bdims[0] {
                let column = index(bx, 0, bz);
                let col = &state[column..column + bdims[1]];
                // The first block that is occupied with an empty block above it, and at least `clear_blocks` of
                // air above it before the next occupied block (or the top of the grid).
                let mut found = None;
                for by in 0..bdims[1] {
                    if col[by] == 0 {
                        continue;
                    }
                    // Real air must lie above the surface. A column solid up to the grid's top has no walkable surface,
                    // so the grid edge never counts as air.
                    let above = (by + 1..bdims[1]).take_while(|&j| col[j] == 0).count();
                    if above >= clear_blocks.max(1) {
                        found = Some(by);
                        break;
                    }
                }
                let Some(by) = found else { continue };
                let origin = [
                    bx * LEAF_SIZE as usize,
                    by * LEAF_SIZE as usize,
                    bz * LEAF_SIZE as usize,
                ];
                if col[by] == 1 {
                    for dz in 0..LEAF_SIZE as usize {
                        for dx in 0..LEAF_SIZE as usize {
                            let (x, z) = (origin[0] + dx, origin[2] + dz);
                            if x < dims[0] as usize && z < dims[2] as usize {
                                floor.push([x as u32, (origin[1] + 3) as u32, z as u32]);
                            }
                        }
                    }
                } else {
                    let mask =
                        self.leaf_mask([origin[0] as u32, origin[1] as u32, origin[2] as u32]);
                    for dz in 0..LEAF_SIZE {
                        for dx in 0..LEAF_SIZE {
                            let top = (0..LEAF_SIZE)
                                .rev()
                                .find(|&dy| mask & (1u64 << (dx + 4 * dy + 16 * dz)) != 0);
                            if let Some(dy) = top {
                                let (x, y, z) = (
                                    origin[0] + dx as usize,
                                    origin[1] + dy as usize,
                                    origin[2] + dz as usize,
                                );
                                if x < dims[0] as usize
                                    && z < dims[2] as usize
                                    && y < dims[1] as usize
                                {
                                    floor.push([x as u32, y as u32, z as u32]);
                                }
                            }
                        }
                    }
                }
            }
        }
        floor
    }
}

fn parse_meta(json: &serde_json::Value) -> Result<Meta, Box<dyn std::error::Error>> {
    let version = json["version"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| json["version"].to_string());
    if !(version == "1" || version.starts_with("1.")) {
        return Err(format!("unsupported voxel version {version}; expected 1.x").into());
    }
    let vec3 = |v: &serde_json::Value| -> Result<[f32; 3], Box<dyn std::error::Error>> {
        let a = v
            .as_array()
            .filter(|a| a.len() == 3)
            .ok_or("expected a 3-element array")?;
        Ok([0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0) as f32))
    };
    let bounds = &json["gridBounds"];
    let grid_min = vec3(&bounds["min"])?;
    let grid_max = vec3(&bounds["max"])?;
    let resolution = json["voxelResolution"]
        .as_f64()
        .ok_or("voxelResolution missing")? as f32;
    if json["leafSize"].as_u64() != Some(LEAF_SIZE as u64) {
        return Err("unsupported leaf size; expected 4".into());
    }
    let tree_depth = json["treeDepth"].as_u64().ok_or("treeDepth missing")? as u32;
    let node_count = json["nodeCount"].as_u64().ok_or("nodeCount missing")? as usize;
    let leaf_data_count = json["leafDataCount"]
        .as_u64()
        .ok_or("leafDataCount missing")? as usize;
    if !leaf_data_count.is_multiple_of(2) {
        return Err("leafDataCount must be even (two words per mixed leaf)".into());
    }
    Ok(Meta {
        grid_min,
        grid_max,
        resolution,
        tree_depth,
        node_count,
        leaf_data_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A depth-1 tree of side 8 cells. Node 0 is interior with only octant 0; node 1 is that leaf, pointing at
    /// mixed leaf 0. The leaf's 64-bit mask has one occupied cell at (1, 2, 3).
    fn tiny_octree() -> Octree {
        let meta = Meta {
            grid_min: [0.0; 3],
            grid_max: [8.0, 8.0, 8.0],
            resolution: 1.0,
            tree_depth: 1,
            node_count: 2,
            leaf_data_count: 2,
        };
        let bit = 1 + 2 * 4 + 3 * 16; // x = 1, y = 2, z = 3 inside the 4×4×4 leaf
        let words = vec![
            (0b1u32 << 24) | 1, // interior: child mask 0b1, first child at node 1
            0,                  // leaf: mixed leaf index 0
            if bit < 32 { 1 << bit } else { 0 },
            if bit >= 32 { 1 << (bit - 32) } else { 0 },
        ];
        Octree { meta, words }
    }

    fn tree_with_cells(cells: &[[u32; 3]]) -> Octree {
        let mut masks = [0u64; 8];
        for &[x, y, z] in cells {
            let octant = (x / 4) | ((y / 4) << 1) | ((z / 4) << 2);
            masks[octant as usize] |= 1 << ((x % 4) + 4 * (y % 4) + 16 * (z % 4));
        }
        let child_mask = masks
            .iter()
            .enumerate()
            .fold(0u32, |m, (i, v)| m | if *v != 0 { 1 << i } else { 0 });
        let count = child_mask.count_ones() as usize;
        let mut words = vec![(child_mask << 24) | 1];
        words.extend(0..count as u32);
        for mask in masks.into_iter().filter(|v| *v != 0) {
            words.extend([mask as u32, (mask >> 32) as u32]);
        }
        Octree {
            meta: Meta {
                grid_min: [0.0; 3],
                grid_max: [0.4; 3],
                resolution: 0.05,
                tree_depth: 1,
                node_count: count + 1,
                leaf_data_count: count * 2,
            },
            words,
        }
    }

    #[test]
    fn fitted_walls_keep_thin_visible_surfaces_without_inflating_a_mixed_leaf() {
        let mut cells: Vec<_> = (0..8)
            .flat_map(|y| (0..4).map(move |z| [2, y, z]))
            .collect();
        cells.push([0, 4, 0]); // isolated scan speckle
        let boxes = tree_with_cells(&cells).wall_boxes(&[Some(0); 4], [2, 2], (0, 2));
        assert!(!boxes.is_empty());
        for b in &boxes {
            assert!(
                (b.min[0] - 0.1).abs() < 1e-5 && (b.max[0] - 0.15).abs() < 1e-5,
                "a 5 cm facade must not become a 20 cm wall: {b:?}"
            );
            assert!(b.max[1] <= 0.4 + 1e-5, "source height, not a 3 m extrusion");
        }
    }

    #[test]
    fn wall_geometry_preserves_clear_air_under_overhangs_and_between_surfaces() {
        let cells: Vec<_> = [1, 6]
            .into_iter()
            .flat_map(|y| (0..4).flat_map(move |x| (0..4).map(move |z| [x, y, z])))
            .collect();
        let boxes = tree_with_cells(&cells).wall_boxes(&[Some(0); 4], [2, 2], (0, 2));
        assert!(!boxes.is_empty());
        assert!(
            boxes
                .iter()
                .all(|b| b.max[1] <= 0.10001 || b.min[1] >= 0.29999),
            "never bridge empty vertical space"
        );
        let noise = tree_with_cells(&[[1, 2, 1]]).wall_boxes(&[Some(0); 4], [2, 2], (0, 2));
        assert!(noise.is_empty(), "an isolated voxel is not a wall");
        let post: Vec<_> = (0..8).map(|y| [1, y, 1]).collect();
        assert!(
            !tree_with_cells(&post)
                .wall_boxes(&[Some(0); 4], [2, 2], (0, 2))
                .is_empty(),
            "retain supported narrow posts"
        );
    }

    #[test]
    fn occupied_cells_decode_the_leaf_mask() {
        let tree = tiny_octree();
        let mut cells = Vec::new();
        tree.for_each_occupied(|c| cells.push(c));
        assert_eq!(cells, vec![[1, 2, 3]]);
    }

    #[test]
    fn solid_nodes_cover_their_whole_span() {
        let meta = Meta {
            grid_min: [0.0; 3],
            grid_max: [4.0; 3],
            resolution: 1.0,
            tree_depth: 0,
            node_count: 1,
            leaf_data_count: 0,
        };
        let tree = Octree {
            meta,
            words: vec![SOLID],
        };
        let mut count = 0;
        tree.for_each_occupied(|_| count += 1);
        assert_eq!(count, 64);
    }

    #[test]
    fn a_solid_block_to_the_grid_top_has_no_walkable_floor() {
        // The block fills the grid to its top, so nothing is air above it: no floor.
        let meta = Meta {
            grid_min: [0.0; 3],
            grid_max: [4.0; 3],
            resolution: 1.0,
            tree_depth: 0,
            node_count: 1,
            leaf_data_count: 0,
        };
        let tree = Octree {
            meta,
            words: vec![SOLID],
        };
        assert!(tree.floor_cells(2).is_empty());
    }

    #[test]
    fn a_lone_cell_is_a_floor_when_nothing_is_above_it() {
        let tree = tiny_octree();
        assert_eq!(tree.floor_cells(2), vec![[1, 2, 3]]);
        assert!(tree.floor_cells(2).len() == 1);
    }

    #[test]
    fn rejects_a_binary_of_the_wrong_length() {
        let meta = Meta {
            grid_min: [0.0; 3],
            grid_max: [4.0; 3],
            resolution: 1.0,
            tree_depth: 0,
            node_count: 1,
            leaf_data_count: 0,
        };
        assert!(Octree::from_bytes(meta, &[0u8; 8]).is_err());
    }
}
