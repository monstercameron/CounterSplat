use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{SceneManifest, Skybox};
use serde_json::json;

fn fixture() -> serde_json::Value {
    json!({
        "version": 1, "name": "Test scene",
        "component_types": {"Skybox": {"storage":"native"}, "Position":{"storage":"dynamic"}},
        "resources": {
            "assets": {"sky": {"kind":"radiance_hdr", "uri":"sky.hdr"}},
            "scripts": {"motion": {"language":"lua", "uri":"motion.lua", "autoload":false}},
            "game": {"gravity": 9.8}
        },
        "entities": [
            {"id":"root", "layer":"scene", "components":{"Skybox":{"asset":"sky"}}},
            {"id":"child", "layer":"scene", "parent":"root", "components":{"Position":{"x":3}}},
            {"id":"overlay", "layer":"ui", "components":{"Position":{"x":4}}}
        ]
    })
}
fn parse(value: serde_json::Value) -> Result<SceneManifest, String> {
    SceneManifest::from_json(&serde_json::to_vec(&value).unwrap())
}

#[test]
fn layer_instantiation_creates_components_hierarchy_and_script_visible_resources() {
    let scene = parse(fixture()).unwrap();
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layer(&mut world, &scene, "scene").unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(world.parent(ids["child"]), Some(ids["root"]));
    assert_eq!(world.get::<Skybox>(ids["root"]).unwrap().asset, "sky");
    assert_eq!(world.get_dynamic(ids["child"], "Position").unwrap()["x"], 3);
    assert_eq!(world.get_dynamic_resource("game").unwrap()["gravity"], 9.8);
    assert_eq!(
        world.get_dynamic_resource("scene.scripts").unwrap()["motion"]["uri"],
        "motion.lua"
    );
    assert_eq!(
        world.get_dynamic_resource("scene.entities").unwrap()["root"],
        ids["root"].to_string()
    );
}

#[test]
fn multiple_layers_share_one_world_and_entity_resource() {
    let scene = parse(fixture()).unwrap();
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let ids =
        countersplat_scene_ecs::instantiate_layers(&mut world, &scene, &["scene", "ui"]).unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(world.parent(ids["child"]), Some(ids["root"]));
    assert_eq!(
        world.get_dynamic_resource("scene.entities").unwrap()["overlay"],
        ids["overlay"].to_string()
    );
    assert_eq!(
        world.get_dynamic(ids["overlay"], "Position").unwrap()["x"],
        4
    );
}

#[test]
fn rejects_invalid_declarations_hierarchy_and_references() {
    let mut bad = fixture();
    bad["version"] = json!(2);
    assert!(parse(bad).unwrap_err().contains("version"));
    let mut bad = fixture();
    bad["entities"][1]["id"] = json!("root");
    assert!(parse(bad).unwrap_err().contains("duplicate"));
    let mut bad = fixture();
    bad["entities"][0]["parent"] = json!("child");
    assert!(parse(bad).unwrap_err().contains("cycle"));
    let mut bad = fixture();
    bad["entities"][1]["parent"] = json!("overlay");
    assert!(parse(bad).unwrap_err().contains("another layer"));
    let mut bad = fixture();
    bad["entities"][1]["components"]["Undeclared"] = json!({});
    assert!(parse(bad).unwrap_err().contains("undeclared"));
    let mut bad = fixture();
    bad["entities"][0]["components"]["Skybox"]["asset"] = json!("missing");
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    assert!(
        countersplat_scene_ecs::instantiate_layer(&mut world, &parse(bad).unwrap(), "scene")
            .unwrap_err()
            .contains("missing skybox asset")
    );
    assert!(world.is_empty());
    assert!(world.get_dynamic_resource("scene.assets").is_none());
}

struct Reject;
impl ComponentAdapter for Reject {
    fn read(&self, _: &World, _: Entity) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
        Err("rejected by adapter".into())
    }
    fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn contains(&self, _: &World, _: Entity) -> bool {
        false
    }
}

#[test]
fn failed_adapter_write_rolls_back_entities_and_does_not_publish_resources() {
    let mut bad = fixture();
    bad["component_types"]["ZReject"] = json!({"storage":"native"});
    bad["entities"][1]["components"]["ZReject"] = json!({});
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    world.register_component_adapter("ZReject", Reject).unwrap();
    let existing = world.spawn();
    let error =
        countersplat_scene_ecs::instantiate_layer(&mut world, &parse(bad).unwrap(), "scene")
            .unwrap_err();
    assert!(error.contains("rejected by adapter"));
    assert_eq!(world.entities(), vec![existing]);
    assert!(world.get_dynamic_resource("game").is_none());
    assert!(world.get_dynamic_resource("scene.entities").is_none());
}

#[test]
fn native_component_requires_an_adapter_and_resource_collisions_are_rejected() {
    let scene = parse(fixture()).unwrap();
    let mut world = World::new();
    assert!(
        countersplat_scene_ecs::instantiate_layer(&mut world, &scene, "scene")
            .unwrap_err()
            .contains("adapter is not registered")
    );
    countersplat_scene_ecs::register(&mut world).unwrap();
    world.set_dynamic_resource("game", json!({"original":true}));
    assert!(
        countersplat_scene_ecs::instantiate_layer(&mut world, &scene, "scene")
            .unwrap_err()
            .contains("already exists")
    );
    assert!(world.is_empty());
    assert_eq!(
        world.get_dynamic_resource("game").unwrap()["original"],
        true
    );
}
