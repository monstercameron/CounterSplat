use countersplat_physics::voxel_collision::VoxelCollider;
use serde_json::json;

fn collider(version: &str) -> VoxelCollider {
    // A one-block octree with a one-voxel-thick wall at x=2.
    let metadata = json!({"version":version,"gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for z in 0..4 {
        for y in 0..4 {
            mask |= 1 << (2 + y * 4 + z * 16);
        }
    }
    let words = [0u32, mask as u32, (mask >> 32) as u32];
    let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap()
}
#[test]
fn sphere_hits_the_wall_without_tunneling_and_can_slide() {
    let world = collider("1.1");
    let hit = world
        .move_sphere([1.0, 1.5, 1.0], [2.0, 0.0, 1.0], 0.2)
        .unwrap();
    assert!(hit[0] <= 1.801, "wall must block the sphere: {hit:?}");
    assert!(hit[2] > 1.7, "tangential movement should slide: {hit:?}");
    assert!(world
        .move_sphere([1.0, 1.5, 1.0], [1000.0, 0.0, 0.0], 0.2)
        .is_err());
}
#[test]
fn debug_samples_are_bounded_and_legacy_data_flips_xy() {
    let world = collider("1.0");
    let hit = world
        .move_sphere([-1.0, -1.5, 1.0], [-2.0, 0.0, 0.0], 0.2)
        .unwrap();
    assert!(
        hit[0] >= -1.801,
        "legacy X/Y coordinates must be converted: {hit:?}"
    );
    let samples = world.nearby_centers([-2.5, -1.5, 1.5], 3.0, 4).unwrap();
    assert_eq!(samples.len(), 4);
    assert!(samples.iter().all(|p| p[0] < 0.0 && p[1] < 0.0));
}

#[test]
fn ground_probe_finds_a_supported_floor_and_respects_search_distance() {
    let metadata = json!({"version":"1.1","gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + z * 16);
        }
    }
    let bytes: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let world = VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap();
    assert_eq!(
        world.ground_height([1.5, 2.4, 1.5], 0.1, 2.0).unwrap(),
        Some(1.0)
    );
    assert_eq!(
        world.ground_height([1.5, 2.4, 1.5], 0.1, 1.0).unwrap(),
        None
    );
    assert!(world.ground_height([1.5, 2.4, 1.5], 0.1, f32::NAN).is_err());
}

#[test]
fn ground_probe_respects_legacy_coordinate_flip() {
    let metadata = json!({"version":"1.0","gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + 3 * 4 + z * 16);
        }
    }
    let bytes: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let world = VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap();
    assert_eq!(
        world.ground_height([-1.5, -2.4, 1.5], 0.1, 1.0).unwrap(),
        Some(-3.0)
    );
}
