//! A loose octree over immutable level-region bounds. The per-region streaming state stays in LevelStream;
//! this index tracks selected region IDs so a region leaving the query can be retained and faded safely.
#[cfg(test)]
use super::selection::forward_ellipse_intersects;
use super::selection::ForwardEllipseQuery;
use crate::lod::Leaf;

const NODE_CAPACITY: usize = 12;
const MAX_DEPTH: usize = 12;

struct Node {
    min: [f32; 3],
    max: [f32; 3],
    children: [Option<usize>; 8],
    regions: Vec<usize>,
}

pub(super) struct SpatialOctree {
    nodes: Vec<Node>,
    // Regions selected last frame, including those that may need a delayed retirement.
    active: Vec<usize>,
    // Reused traversal stack: level geometry is immutable, so it retains a bounded high-water mark.
    stack: Vec<usize>,
}
impl SpatialOctree {
    pub fn new(leaves: &[Leaf]) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            active: (0..leaves.len()).collect(),
            stack: Vec::new(),
        };
        if !leaves.is_empty() {
            tree.build(leaves, (0..leaves.len()).collect(), 0);
        }
        tree
    }
    fn build(&mut self, leaves: &[Leaf], regions: Vec<usize>, depth: usize) -> usize {
        let min = [0, 1, 2].map(|axis| {
            regions
                .iter()
                .map(|&i| leaves[i].min[axis])
                .fold(f32::INFINITY, f32::min)
        });
        let max = [0, 1, 2].map(|axis| {
            regions
                .iter()
                .map(|&i| leaves[i].max[axis])
                .fold(f32::NEG_INFINITY, f32::max)
        });
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            children: [None; 8],
            regions: Vec::new(),
        });
        if regions.len() <= NODE_CAPACITY || depth >= MAX_DEPTH {
            self.nodes[index].regions = regions;
            return index;
        }
        let midpoint = [0, 1, 2].map(|axis| (min[axis] + max[axis]) * 0.5);
        let mut groups: [Vec<usize>; 8] = Default::default();
        for &region in &regions {
            let leaf = &leaves[region];
            let mut octant = 0;
            for axis in 0..3 {
                if (leaf.min[axis] + leaf.max[axis]) * 0.5 >= midpoint[axis] {
                    octant |= 1 << axis;
                }
            }
            groups[octant].push(region);
        }
        // Coincident centers cannot be partitioned; stop rather than building an empty chain.
        if groups.iter().filter(|group| !group.is_empty()).count() <= 1 {
            self.nodes[index].regions = regions;
            return index;
        }
        for (octant, group) in groups.into_iter().enumerate() {
            if !group.is_empty() {
                let child = self.build(leaves, group, depth + 1);
                self.nodes[index].children[octant] = Some(child);
            }
        }
        index
    }
    /// Candidate regions in the wider retention oval, plus previous active regions needing retirement.
    pub fn forward_candidates(&mut self, query: &ForwardEllipseQuery, out: &mut Vec<usize>) {
        out.clear();
        out.extend_from_slice(&self.active);
        self.stack.clear();
        if !self.nodes.is_empty() {
            self.stack.push(0);
        }
        while let Some(index) = self.stack.pop() {
            let node = &self.nodes[index];
            if !query.intersects(node.min, node.max) {
                continue;
            }
            out.extend_from_slice(&node.regions);
            self.stack.extend(node.children.iter().flatten().copied());
        }
        out.sort_unstable();
        out.dedup();
    }
    pub fn set_active(&mut self, active: impl IntoIterator<Item = usize>) {
        self.active.clear();
        self.active.extend(active);
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The previous allocation-based traversal is kept here as an independent ordering and
    // retention reference. The production path reuses buffers but must return the same IDs.
    fn reference(
        tree: &SpatialOctree,
        eye: [f32; 3],
        forward: [f32; 3],
        reach: f32,
        rear: f32,
        side: f32,
    ) -> Vec<usize> {
        let mut out = tree.active.clone();
        let mut stack = if tree.nodes.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
        while let Some(index) = stack.pop() {
            let node = &tree.nodes[index];
            if forward_ellipse_intersects(eye, forward, node.min, node.max, reach, rear, side, 1.20)
            {
                out.extend_from_slice(&node.regions);
                stack.extend(node.children.iter().flatten().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn check_sequence(leaves: &[Leaf]) {
        let mut tree = SpatialOctree::new(leaves);
        let mut out = vec![usize::MAX]; // stale buffer contents must never leak into a new query.
        for frame in 0..256 {
            let angle = frame as f32 * 0.079;
            let eye = [
                (frame % 31) as f32 * 4.0 - 40.0,
                1.0,
                (frame % 23) as f32 * 3.0 - 20.0,
            ];
            let direction = [angle.cos(), 0.0, angle.sin()];
            let reach = [0.0, 15.0, 70.0, 300.0][frame % 4];
            let expected = reference(&tree, eye, direction, reach, 0.3, 0.55);
            let query = ForwardEllipseQuery::new(eye, direction, reach, 0.3, 0.55, 1.20);
            tree.forward_candidates(&query, &mut out);
            assert_eq!(out, expected, "frame {frame}");
            // Change which regions must be revisited, including occasional duplicates and no active regions.
            let active = out
                .iter()
                .copied()
                .filter(|id| (id + frame) % 4 == 0)
                .chain((frame % 19 == 0 && !leaves.is_empty()).then_some(0))
                .collect::<Vec<_>>();
            tree.set_active(active);
        }
    }

    #[test]
    fn reused_query_buffers_match_previous_candidate_order_and_retention() {
        let grid: Vec<_> = (0..400)
            .map(|i| {
                let x = (i % 20) as f32 * 8.0 - 80.0;
                let z = (i / 20) as f32 * 8.0 - 80.0;
                Leaf {
                    min: [x, -1.0, z],
                    max: [x + 7.0, 3.0, z + 7.0],
                    lods: Vec::new(),
                }
            })
            .collect();
        check_sequence(&grid);
        check_sequence(&[]);
        // Coincident centers cannot subdivide, and still need to use the same retention rules.
        check_sequence(&vec![grid[0].clone(); 30]);
    }
}
