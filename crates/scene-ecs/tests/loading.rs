use countersplat_ecs::World;
use countersplat_scene::{AssetLoader, AssetLoadingStatus};
use serde_json::json;

#[test]
fn loading_adapter_exposes_validated_progress_to_scripts() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world
        .write_component_value(
            entity,
            "AssetLoader",
            json!({
                "status":"loading", "phase":"Textures", "progress":0.5, "completed":2, "total":4
            }),
        )
        .unwrap();
    let before = world.get::<AssetLoader>(entity).unwrap().clone();
    assert_eq!(before.status, AssetLoadingStatus::Loading);
    assert_eq!(
        world
            .read_component_value(entity, "AssetLoader")
            .unwrap()
            .unwrap()["progress"],
        json!(0.5)
    );
    assert!(world
        .write_component_value(
            entity,
            "AssetLoader",
            json!({
                "status":"loading", "progress":1.2, "completed":5, "total":4
            })
        )
        .is_err());
    assert_eq!(*world.get::<AssetLoader>(entity).unwrap(), before);
}
