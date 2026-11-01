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

fn capsule_world(mask: u64, version: &str) -> VoxelCollider {
    let metadata = json!({"version":version,"gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let bytes: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap()
}

#[test]
fn upright_capsule_collides_at_body_height_instead_of_only_at_the_feet() {
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + z * 16);
        }
        mask |= 1 << (2 + 2 * 4 + z * 16); // chest-height wall
    }
    let world = capsule_world(mask, "1.1");
    let start = [1.0, 1.8, 1.5];
    let delta = [2.0, 0.0, 0.0];
    let foot_sphere = world.move_sphere(start, delta, 0.2).unwrap();
    let upright = world.move_capsule(start, delta, 0.2, 0.6).unwrap();
    assert!(
        foot_sphere[0] > 2.5,
        "foot sphere passes under wall: {foot_sphere:?}"
    );
    assert!(
        upright[0] <= 1.801,
        "upright body must stop at wall: {upright:?}"
    );
    assert!(
        (upright[1] - 1.8).abs() < 0.01,
        "floor height is preserved: {upright:?}"
    );
}

#[test]
fn upright_capsule_stands_on_the_floor_and_stops_at_the_ceiling() {
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + z * 16); // floor y=0
            mask |= 1 << (x + 3 * 4 + z * 16); // ceiling y=3
        }
    }
    let world = capsule_world(mask, "1.1");
    let landed = world
        .move_capsule([1.5, 2.3, 1.5], [0.0, -0.8, 0.0], 0.2, 0.6)
        .unwrap();
    assert!(
        (landed[1] - 1.8).abs() < 0.01,
        "capsule bottom rests at y=1: {landed:?}"
    );
    let head_hit = world
        .move_capsule(landed, [0.0, 1.0, 0.0], 0.2, 0.6)
        .unwrap();
    assert!(
        (head_hit[1] - 2.2).abs() < 0.01,
        "capsule top stops at y=3: {head_hit:?}"
    );
    assert_eq!(
        world
            .move_sphere([1.5, 2.0, 1.5], [0.1, -0.2, 0.0], 0.2)
            .unwrap(),
        world
            .move_capsule([1.5, 2.0, 1.5], [0.1, -0.2, 0.0], 0.2, 0.0)
            .unwrap()
    );
    assert!(world
        .move_capsule([1.0; 3], [0.0, 0.1, 0.0], 0.2, f32::NAN)
        .is_err());
}

#[test]
fn upright_capsule_respects_legacy_coordinate_rotation() {
    let mut mask = 0u64;
    for z in 0..4 {
        mask |= 1 << (2 + 2 * 4 + z * 16);
    }
    let world = capsule_world(mask, "1.0");
    let hit = world
        .move_capsule([-1.0, -1.8, 1.5], [-2.0, 0.0, 0.0], 0.2, 0.6)
        .unwrap();
    assert!(
        hit[0] >= -1.801,
        "legacy capsule must stop at the wall: {hit:?}"
    );
}

#[test]
fn capsule_clearance_blocks_a_low_ceiling_and_accepts_touching_floor() {
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + z * 16);
            mask |= 1 << (x + 3 * 4 + z * 16);
        }
    }
    let world = capsule_world(mask, "1.1");
    // Crouched player fits between the floor at y=1 and ceiling at y=3.
    assert!(world.capsule_clear([1.5, 1.65, 1.5], 0.2, 0.45).unwrap());
    // The standing top intersects the same ceiling.
    assert!(!world.capsule_clear([1.5, 2.075, 1.5], 0.2, 0.875).unwrap());
    // The top-sphere sweep used by the posture script also detects it.
    assert!(!world.capsule_clear([1.5, 2.525, 1.5], 0.2, 0.625).unwrap());
    assert!(world
        .capsule_clear([1.5, 1.65, 1.5], f32::NAN, 0.45)
        .is_err());
    let legacy = capsule_world(mask, "1.0");
    assert!(legacy.capsule_clear([-1.5, -1.65, 1.5], 0.2, 0.45).unwrap());
    assert!(!legacy
        .capsule_clear([-1.5, -2.075, 1.5], 0.2, 0.875)
        .unwrap());
}

#[test]
fn segment_tracing_hits_the_first_voxel_without_tunneling() {
    let world = collider("1.1");
    let hit = world
        .trace_segment([-10.0, 1.5, 1.5], [20.0, 1.5, 1.5])
        .unwrap()
        .unwrap();
    assert!((hit.fraction - 0.4).abs() < 0.00001);
    assert_eq!(hit.position, [2.0, 1.5, 1.5]);
    assert_eq!(hit.normal, [-1.0, 0.0, 0.0]);
    let reverse = world
        .trace_segment([3.9, 1.5, 1.5], [0.0, 1.5, 1.5])
        .unwrap()
        .unwrap();
    assert!((reverse.position[0] - 3.0).abs() < 0.0001);
    assert_eq!(reverse.normal, [1.0, 0.0, 0.0]);
    assert!(world
        .trace_segment([0.1, 5.0, 1.5], [3.9, 5.0, 1.5])
        .unwrap()
        .is_none());
    assert!(world
        .trace_segment([0.1, 1.5, 1.5], [1.9, 1.5, 1.5])
        .unwrap()
        .is_none());
    assert_eq!(
        world
            .trace_segment([2.5, 1.5, 1.5], [2.5, 1.5, 1.5])
            .unwrap()
            .unwrap()
            .fraction,
        0.0
    );
    assert!(world.trace_segment([f32::NAN, 0.0, 0.0], [0.0; 3]).is_err());
}

#[test]
fn segment_tracing_respects_negative_boundaries_and_legacy_coordinates() {
    let world = collider("1.1");
    assert!(world
        .trace_segment([2.0, 1.5, 1.5], [1.0, 1.5, 1.5])
        .unwrap()
        .is_none());
    assert_eq!(
        world
            .trace_segment([2.0, 1.5, 1.5], [3.0, 1.5, 1.5])
            .unwrap()
            .unwrap()
            .fraction,
        0.0
    );
    let hit = collider("1.0")
        .trace_segment([0.0, -1.5, 1.5], [-4.0, -1.5, 1.5])
        .unwrap()
        .unwrap();
    assert_eq!(hit.position, [-2.0, -1.5, 1.5]);
    assert_eq!(hit.normal, [1.0, 0.0, 0.0]);
    let diagonal = world
        .trace_segment([0.0, 0.0, 0.0], [4.0, 4.0, 4.0])
        .unwrap()
        .unwrap();
    assert_eq!(diagonal.position, [2.0, 2.0, 2.0]);
}
