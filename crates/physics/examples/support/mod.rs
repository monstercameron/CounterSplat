use countersplat_physics::voxel_collision::VoxelCollider;
use serde_json::json;

pub fn synthetic() -> VoxelCollider {
    const DEPTH: u32 = 4;
    fn node(
        index: usize,
        depth: u32,
        origin: [u32; 3],
        nodes: &mut Vec<u32>,
        leaves: &mut Vec<u32>,
    ) {
        if depth == DEPTH {
            let mut mask = 0_u64;
            for z in 0..4 {
                for y in 0..4 {
                    for x in 0..4 {
                        let [gx, gy, gz] = [origin[0] + x, origin[1] + y, origin[2] + z];
                        let floor = gy < 2 && !(gx >= 20 && gx < 26 && gz >= 16 && gz < 22);
                        let wall = gx >= 30 && gx < 32 && gy < 40 && gz >= 8 && gz < 54;
                        let box_shape = (45..49).contains(&gx)
                            && (2..14).contains(&gy)
                            && (25..35).contains(&gz);
                        if floor || wall || box_shape {
                            mask |= 1_u64 << (x + 4 * y + 16 * z);
                        }
                    }
                }
            }
            nodes[index] = (leaves.len() / 2) as u32;
            leaves.extend([mask as u32, (mask >> 32) as u32]);
        } else {
            let first = nodes.len();
            nodes.resize(first + 8, 0);
            nodes[index] = 0xff00_0000 | first as u32;
            let child_span = 4 << (DEPTH - depth - 1);
            for octant in 0..8 {
                let child =
                    [0, 1, 2].map(|axis| origin[axis] + ((octant >> axis) & 1) as u32 * child_span);
                node(first + octant, depth + 1, child, nodes, leaves);
            }
        }
    }
    let mut nodes = vec![0];
    let mut leaves = Vec::new();
    node(0, 0, [0; 3], &mut nodes, &mut leaves);
    let metadata = json!({"version":"1.1", "gridBounds":{"min":[0,0,0],"max":[3.2,3.2,3.2]},
        "voxelResolution":0.05, "leafSize":4, "treeDepth":DEPTH,
        "nodeCount":nodes.len(), "leafDataCount":leaves.len()});
    let bytes: Vec<_> = nodes
        .into_iter()
        .chain(leaves)
        .flat_map(u32::to_le_bytes)
        .collect();
    VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap()
}
