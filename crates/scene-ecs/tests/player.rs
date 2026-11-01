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

#[test]
fn camera_field_reads_match_the_exposed_component() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let player = world.spawn();
    let mut camera = PlayerCamera::default();
    camera.position = [0.1, -0.2, 0.3];
    camera.view_turn_roll = 0.1;
    camera.fov_degrees = 71.3;
    world.insert(player, camera).unwrap();
    let full = world
        .read_component_value(player, "PlayerCamera")
        .unwrap()
        .unwrap();
    for field in [
        "position",
        "view_offset",
        "view_posture_offset_y",
        "view_bob_pitch",
        "view_bob_roll",
        "view_turn_roll",
        "yaw",
        "pitch",
        "roll",
        "fov_degrees",
        "view_fov_degrees",
        "missing",
    ] {
        assert_eq!(
            world
                .read_component_field(player, "PlayerCamera", field)
                .unwrap(),
            full.get(field).cloned(),
            "{field}"
        );
    }
    let id = player.to_string();
    let full_json: serde_json::Value = serde_json::from_str(
        &world
            .apply_script_json(&json!({"op":"get", "id":id, "name":"PlayerCamera"}).to_string())
            .unwrap(),
    )
    .unwrap();
    for field in [
        "position",
        "view_turn_roll",
        "fov_degrees",
        "view_fov_degrees",
    ] {
        let direct = world
            .script_component_field_json(&id, "PlayerCamera", field)
            .unwrap()
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(
            &world
                .apply_script_json(
                    &json!({"op":"get_field", "id":id, "name":"PlayerCamera", "field":field})
                        .to_string(),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&direct).unwrap(),
            full_json["value"][field],
            "direct {field}"
        );
        assert_eq!(result["value"], full_json["value"][field], "JSON {field}");
    }
    let mut controls = PlayerControls::default();
    controls.move_speed = 4.2;
    world.insert(player, controls).unwrap();
    let full_controls: serde_json::Value = serde_json::from_str(
        &world
            .apply_script_json(&json!({"op":"get", "id":id, "name":"PlayerControls"}).to_string())
            .unwrap(),
    )
    .unwrap();
    let field = world
        .script_component_field_json(&id, "PlayerControls", "move_speed")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&field).unwrap(),
        full_controls["value"]["move_speed"]
    );
    assert_eq!(
        world
            .read_component_field(player, "PlayerCamera", "view_fov_degrees")
            .unwrap(),
        Some(json!(null))
    );
    world.remove::<PlayerCamera>(player);
    assert_eq!(
        world
            .read_component_field(player, "PlayerCamera", "yaw")
            .unwrap(),
        None
    );
}
