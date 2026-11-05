//! Shared fixtures for the scene integration tests.
use std::path::Path;

/// The shipped gameplay scene with the weapon ready and drawn at spawn. While its footage is being remade the scene
/// spawns with the weapon stowed (`WeaponConfig.start_stowed`) and the first-person view off
/// (`FirstPersonView.enabled`), so shots go through the crosshair; tests of weapon, camera and HUD behaviour start
/// from a ready, visible weapon, and `first_person_lua.rs` covers the shipped spawn.
pub fn gameplay_with_ready_weapon(root: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(root.join("assets/scenes/gameplay.json")).unwrap();
    let mut scene: serde_json::Value = serde_json::from_str(&text).unwrap();
    let weapon = scene["entities"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entity| entity["id"] == "first_person")
        .expect("the gameplay scene's first_person entity");
    let components = &mut weapon["components"];
    assert_eq!(components["WeaponConfig"]["start_stowed"], true, "the gameplay scene's spawn setting moved");
    components["WeaponConfig"]["start_stowed"] = false.into();
    components["WeaponState"]["state"] = "lowReady".into();
    components["FirstPersonView"]["enabled"] = true.into();
    serde_json::to_vec(&scene).unwrap()
}
