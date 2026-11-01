use countersplat_ecs::World;
use countersplat_scene::{LevelAssets, LevelFormat, LevelSettings};
use serde_json::json;

#[test]
fn level_adapters_validate_asset_paths_and_available_lods() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    let assets = LevelAssets {
        level_id: "1234abcd".into(),
        name: "Example".into(),
        format: Some(LevelFormat::StreamedSog),
        lod_count: 3,
        ..Default::default()
    };
    world
        .write_component_value(entity, "LevelAssets", serde_json::to_value(assets).unwrap())
        .unwrap();
    world
        .write_component_value(entity, "LevelSettings", json!({"lod":2}))
        .unwrap();
    assert!(world
        .write_component_value(entity, "LevelSettings", json!({"lod":3}))
        .is_err());
    assert_eq!(world.get::<LevelSettings>(entity).unwrap().lod, Some(2));
    assert!(world
        .write_component_value(entity, "LevelSettings", json!({"draw_distance":0}))
        .is_err());
    assert!(world
        .write_component_value(entity, "LevelSettings", json!({"draw_distance":1001}))
        .is_err());
    assert_eq!(
        world.get::<LevelSettings>(entity).unwrap().draw_distance,
        150.0
    );
    let mut value = world
        .read_component_value(entity, "LevelAssets")
        .unwrap()
        .unwrap();
    value["files"] = json!([{"path":"../escape", "kind":"other", "load":"streamable"}]);
    assert!(world
        .write_component_value(entity, "LevelAssets", value)
        .is_err());
    assert!(world.get::<LevelAssets>(entity).unwrap().files.is_empty());
}

#[test]
fn level_settings_revision_tracks_changes_and_lifecycle() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world.insert(entity, LevelSettings::default()).unwrap();
    let initial = world
        .read_component_if_changed(entity, "LevelSettings", None)
        .unwrap()
        .unwrap();
    let revision = initial.revision.unwrap();
    assert!(world
        .read_component_if_changed(entity, "LevelSettings", Some(&revision))
        .unwrap()
        .is_none());
    world
        .get_mut::<LevelSettings>(entity)
        .unwrap()
        .draw_distance = 345.0;
    let changed = world
        .read_component_if_changed(entity, "LevelSettings", Some(&revision))
        .unwrap()
        .unwrap();
    assert_eq!(changed.value["draw_distance"], json!(345.0));
    let revision = changed.revision.unwrap();
    world.remove::<LevelSettings>(entity).unwrap();
    let missing = world
        .read_component_if_changed(entity, "LevelSettings", Some(&revision))
        .unwrap()
        .unwrap();
    assert!(!missing.present);
    let revision = missing.revision.unwrap();
    world.insert(entity, LevelSettings::default()).unwrap();
    let restored = world
        .read_component_if_changed(entity, "LevelSettings", Some(&revision))
        .unwrap()
        .unwrap();
    assert!(restored.present);
    assert_eq!(restored.value["draw_distance"], json!(150.0));
}
