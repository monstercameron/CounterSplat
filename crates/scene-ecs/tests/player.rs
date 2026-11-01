use countersplat_ecs::World;
use countersplat_scene::{PlayerCamera, PlayerControls};
use serde_json::json;

#[test]
fn player_adapters_validate_script_updates_without_mutating_previous_values() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let player = world.spawn();
    world
        .write_component_value(player, "PlayerCamera", json!({}))
        .unwrap();
    world
        .write_component_value(player, "PlayerControls", json!({}))
        .unwrap();
    let camera = world.get::<PlayerCamera>(player).unwrap().clone();
    let controls = world.get::<PlayerControls>(player).unwrap().clone();
    assert!(world
        .write_component_value(player, "PlayerCamera", json!({"pitch": 2.0}))
        .is_err());
    assert!(world
        .write_component_value(player, "PlayerControls", json!({"move_speed": -1}))
        .is_err());
    assert!(world
        .write_component_value(
            player,
            "PlayerControls",
            json!({"bindings":{"forward":"invalid key"}})
        )
        .is_err());
    assert_eq!(*world.get::<PlayerCamera>(player).unwrap(), camera);
    assert_eq!(*world.get::<PlayerControls>(player).unwrap(), controls);
}
