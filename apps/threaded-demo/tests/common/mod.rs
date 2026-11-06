//! Shared fixtures for the scene integration tests.
use std::path::Path;

/// The shipped gameplay scene with the weapon ready and drawn at spawn, aiming from the presented sight. The shipped
/// scene shows revolver concept stills and aims through the camera centre (`FirstPersonView.aim_from_camera`)
/// because the stills have no authored sight landmarks; tests of weapon, camera and HUD behaviour exercise the
/// sight-following path, and `first_person_lua.rs` covers the shipped settings.
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
    components["WeaponConfig"]["start_stowed"] = false.into();
    components["WeaponState"]["state"] = "lowReady".into();
    components["FirstPersonView"]["enabled"] = true.into();
    components["FirstPersonView"].as_object_mut().unwrap().remove("aim_from_camera");
    // The projection tests use the generic default ADS landmark; the shipped front-sight landmark has its own test.
    components["FirstPersonView"].as_object_mut().unwrap().remove("ads_sight");
    // Weapon mechanics are tested against the authored select-fire values, not the shipped revolver profile.
    components.as_object_mut().unwrap().remove("WeaponProfile");
    serde_json::to_vec(&scene).unwrap()
}
