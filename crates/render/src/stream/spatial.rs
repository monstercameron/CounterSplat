//! A loose octree over immutable level-region bounds. The per-region streaming state stays in LevelStream;
//! this index tracks selected region IDs so a region leaving the query can be retained and faded safely.
use super::selection::forward_ellipse_intersects;
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
}
impl SpatialOctree {
    pub fn new(leaves: &[Leaf]) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            active: (0..leaves.len()).collect(),
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
    pub fn forward_candidates(
        &self,
        eye: [f32; 3],
        forward: [f32; 3],
        reach: f32,
        rear_fraction: f32,
        side_fraction: f32,
    ) -> Vec<usize> {
        let mut out = self.active.clone();
        let mut stack = if self.nodes.is_empty() {
            Vec::new()
        } else {
            vec![0usize]
        };
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            if !forward_ellipse_intersects(
                eye,
                forward,
                node.min,
                node.max,
                reach,
                rear_fraction,
                side_fraction,
                1.20,
            ) {
                continue;
            }
            out.extend_from_slice(&node.regions);
            stack.extend(node.children.iter().flatten().copied());
        }
        out.sort_unstable();
        out.dedup();
        out
    }
    pub fn set_active(&mut self, active: Vec<usize>) {
        self.active = active;
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}
