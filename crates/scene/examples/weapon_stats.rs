//! Prints the resolved physics of every weapon profile in a scene, to check a new or edited profile.
//!
//! ```text
//! cargo run --release -p countersplat-scene --example weapon_stats -- assets/scenes/gameplay.json
//! ```
use countersplat_scene::weapon_profile::{resolve, scene_profiles};
use countersplat_scene::SceneManifest;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "assets/scenes/gameplay.json".into());
    let scene = SceneManifest::from_json(&std::fs::read(&path).expect("read the scene")).expect("parse the scene");
    let (profiles, tuning) = scene_profiles(&scene.resources.values).expect("valid weapon profiles");
    println!(
        "{:<14} {:>7} {:>9} {:>6} {:>8} {:>8} {:>7} {:>6} {:>6} {:>5} {:>6} {:>6}",
        "profile", "bore in", "payload gr", "ft/s", "ME ftlbf", "free ftlb", "felt", "mitig", "rpm", "cap", "dmg", "kick deg"
    );
    for (id, profile) in &profiles {
        let s = resolve(profile, &tuning).expect("resolve").stats;
        println!(
            "{:<14} {:>7.3} {:>9.1} {:>6.0} {:>8.0} {:>9.1} {:>7.1} {:>5.0}% {:>6.0} {:>5} {:>6.1} {:>8.2}",
            id,
            s.bore_in,
            s.payload_grains,
            s.muzzle_velocity_fps,
            s.muzzle_energy_ftlbf,
            s.free_recoil_ftlbf,
            s.felt_recoil_ftlbf,
            s.recoil_reduction * 100.0,
            s.fire_rate_rpm,
            s.capacity,
            s.damage_per_projectile * s.projectiles as f64,
            s.recoil_vertical_degrees,
        );
    }
}
