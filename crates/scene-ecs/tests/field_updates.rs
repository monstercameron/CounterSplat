use countersplat_ecs::World;
use countersplat_scene::{PhysicsBody, PlayerCamera, Transform};
use serde_json::json;

#[test]
fn validated_field_updates_are_atomic_and_keep_unrelated_fields() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world.insert(entity, PlayerCamera::default()).unwrap();
    world.insert(entity, PhysicsBody::default()).unwrap();
    world.insert(entity, Transform::default()).unwrap();

    world
        .write_component_field(entity, "PlayerCamera", "position", json!([1.0, 2.0, 3.0]))
        .unwrap();
    let camera = world.get::<PlayerCamera>(entity).unwrap().clone();
    assert_eq!(camera.position, [1.0, 2.0, 3.0]);
    assert!(world
        .write_component_field(
            entity,
            "PlayerCamera",
            "position",
            json!([10000001.0, 2.0, 3.0])
        )
        .is_err());
    assert_eq!(world.get::<PlayerCamera>(entity), Some(&camera));
    // Other scalar camera fields also retain normal adapter validation.
    world
        .write_component_field(entity, "PlayerCamera", "yaw", json!(0.5))
        .unwrap();
    assert_eq!(world.get::<PlayerCamera>(entity).unwrap().yaw, 0.5);
    let camera = world.get::<PlayerCamera>(entity).unwrap().clone();
    assert!(world
        .write_component_field(entity, "PlayerCamera", "pitch", json!(3.0))
        .is_err());
    assert_eq!(world.get::<PlayerCamera>(entity), Some(&camera));

    world
        .write_component_field(entity, "PhysicsBody", "velocity", json!([1.0, -2.0, 3.0]))
        .unwrap();
    let body = world.get::<PhysicsBody>(entity).unwrap().clone();
    assert_eq!(body.velocity, [1.0, -2.0, 3.0]);
    assert!(world
        .write_component_field(entity, "PhysicsBody", "velocity", json!([201.0, 0.0, 0.0]))
        .is_err());
    assert!(world
        .write_component_field(entity, "PhysicsBody", "gravity", json!(-1.0))
        .is_err());
    assert_eq!(world.get::<PhysicsBody>(entity), Some(&body));

    world
        .write_component_field(entity, "Transform", "position", json!([3.0, 2.0, 1.0]))
        .unwrap();
    assert_eq!(
        world.get::<Transform>(entity).unwrap().position,
        [3.0, 2.0, 1.0]
    );
    assert!(world
        .write_component_field(entity, "Transform", "position", json!([0.0, 0.0]))
        .is_err());
    assert_eq!(
        world.get::<Transform>(entity).unwrap().position,
        [3.0, 2.0, 1.0]
    );
}

#[test]
fn multiple_native_fields_commit_together_or_remain_unchanged() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world.insert(entity, PhysicsBody::default()).unwrap();
    world.insert(entity, PlayerCamera::default()).unwrap();
    let fields = json!({"velocity":[0.0,0.0,0.0],"grounded":true})
        .as_object()
        .unwrap()
        .clone();
    world
        .write_component_fields(entity, "PhysicsBody", fields)
        .unwrap();
    let body = world.get::<PhysicsBody>(entity).unwrap().clone();
    assert!(body.grounded);
    let invalid = json!({"velocity":[300.0,0.0,0.0],"grounded":false})
        .as_object()
        .unwrap()
        .clone();
    assert!(world
        .write_component_fields(entity, "PhysicsBody", invalid)
        .is_err());
    assert_eq!(world.get::<PhysicsBody>(entity), Some(&body));
    let unsupported = json!({"grounded":false,"unknown":true})
        .as_object()
        .unwrap()
        .clone();
    assert!(world
        .write_component_fields(entity, "PhysicsBody", unsupported)
        .is_err());
    assert_eq!(world.get::<PhysicsBody>(entity), Some(&body));
    let before = world.get::<PlayerCamera>(entity).unwrap().clone();
    let invalid = json!({"view_offset":[1.0,0.0,0.0],"view_bob_pitch":0.5})
        .as_object()
        .unwrap()
        .clone();
    assert!(world
        .write_component_fields(entity, "PlayerCamera", invalid)
        .is_err());
    assert_eq!(world.get::<PlayerCamera>(entity), Some(&before));
    let fields = json!({"yaw":1.0,"pitch":0.25}).as_object().unwrap().clone();
    world
        .write_component_fields(entity, "PlayerCamera", fields)
        .unwrap();
    assert_eq!(world.get::<PlayerCamera>(entity).unwrap().yaw, 1.0);
    assert_eq!(world.get::<PlayerCamera>(entity).unwrap().pitch, 0.25);
}
