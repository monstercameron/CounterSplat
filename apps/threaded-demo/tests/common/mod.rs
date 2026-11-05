//! Shared fixtures for the scene integration tests.
use std::path::Path;

/// The shipped gameplay scene with the weapon ready at spawn. The scene itself spawns with the weapon stowed
/// (`WeaponConfig.start_stowed`) while its footage is being remade; tests of weapon, camera and HUD behaviour start
/// from a ready weapon, and `first_person_lua.rs` covers the stowed spawn.
pub fn gameplay_with_ready_weapon(root: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(root.join("assets/scenes/gameplay.json")).unwrap();
    // The setting and the matching initial WeaponState (read by camera and HUD scripts before the controller runs).
    for setting in ["\"start_stowed\": true", "\"state\": \"noGun\""] {
        assert_eq!(text.matches(setting).count(), 1, "the gameplay scene's spawn setting moved: {setting}");
    }
    text.replace("\"start_stowed\": true", "\"start_stowed\": false")
        .replace("\"state\": \"noGun\"", "\"state\": \"lowReady\"")
        .into_bytes()
}
