//! Host-side preference rules the browser persists: UI layout keys and their gameplay migration, the saved player
//! controls (with the sprint-default migration), and the order of gameplay event channels.

use serde_json::{Map, Value};

/// Gameplay tool panels whose saved positions reset with layout version 2; other HUD placements are kept.
const GAMEPLAY_TOOLS: [&str; 9] = [
    "scene_panel", "stats_panel", "sky_panel", "controls_panel", "level_panel",
    "entity_inspector", "ecs_panel", "camera_panel", "image_panel",
];

/// Channels dispatched to gameplay scripts before drawing (input), in contract order.
pub const GAMEPLAY_INPUT_CHANNELS: [&str; 4] = ["input.key", "input.mouse_delta", "input.mouse_button", "input.reset"];

/// The remaining gameplay channels, dispatched after drawing in this order.
pub const GAMEPLAY_EVENT_CHANNELS: [&str; 21] = [
    "ui.action", "editor.entity_selected", "physics.ready", "physics.ground_result",
    "physics.clearance_result", "physics.trace_result", "physics.motion_resolved", "physics.motion_error",
    "physics.collision", "physics.error", "combat.reset", "weapon.trigger", "weapon.aim", "weapon.reload",
    "weapon.equip", "weapon.optic_set", "weapon.mode", "weapon.hit", "weapon.die", "weapon.reset", "weapon.posture",
];

/// Storage key for a scene's UI layout (gameplay is on version 2).
pub fn layout_key(scene: &str) -> String {
    format!("countersplat.ui-layout.{scene}.{}", if scene == "gameplay" { "v2" } else { "v1" })
}

fn without_tools(map: &Map<String, Value>) -> Map<String, Value> {
    map.iter()
        .filter(|(id, _)| !GAMEPLAY_TOOLS.contains(&id.as_str()))
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect()
}

/// Migrates a version 1 gameplay layout (flat offsets, or the offset/size format) to version 2 by dropping the tool
/// panel placements. `None` when there is nothing usable.
pub fn migrate_gameplay_layout(raw: Option<&str>) -> Option<String> {
    let saved: Value = serde_json::from_str(raw?).ok()?;
    let object = saved.as_object()?;
    let migrated = match object.get("version") {
        Some(Value::Number(version)) => {
            let (offsets, sizes) = (object.get("offsets")?.as_object()?, object.get("sizes")?.as_object()?);
            if version.as_f64() != Some(2.0) {
                return None;
            }
            let mut out = object.clone();
            out.insert("offsets".into(), Value::Object(without_tools(offsets)));
            out.insert("sizes".into(), Value::Object(without_tools(sizes)));
            out
        }
        _ => without_tools(object),
    };
    serde_json::to_string(&Value::Object(migrated)).ok()
}

pub const PLAYER_CONTROLS_KEY: &str = "countersplat.player-controls.v2";
pub const LEGACY_PLAYER_CONTROLS_KEY: &str = "countersplat.player-controls.v1";

/// Saved player controls (`current`, else the version 1 `legacy` entry). Version 1 used a 3x sprint default:
/// migrating replaces exactly that value with the scene's default, keeping every custom value. Returns the settings
/// and whether they were migrated, or `None` when nothing is saved.
pub fn read_controls_preference(
    current: Option<&str>,
    legacy: Option<&str>,
    scene_sprint_default: Option<f64>,
) -> Result<Option<(Value, bool)>, String> {
    let migrated = current.is_none() && legacy.is_some();
    let Some(raw) = (if migrated { legacy } else { current }) else {
        return Ok(None);
    };
    if raw.len() > 16384 {
        return Err("Saved controls are too large or invalid.".into());
    }
    let mut settings: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let Some(object) = settings.as_object_mut() else {
        return Err("Saved controls must be an object.".into());
    };
    if migrated && object.get("sprint_multiplier").and_then(Value::as_f64) == Some(3.0) {
        if let Some(default) = scene_sprint_default.filter(|d| d.is_finite()) {
            object.insert("sprint_multiplier".into(), default.into());
        }
    }
    Ok(Some((settings, migrated)))
}

/// Storage key of a level's saved player rig.
pub fn player_rig_key(level_id: &str) -> String {
    format!("countersplat.player-rig.v1.{level_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layout_keys_and_gameplay_migration() {
        assert_eq!(layout_key("gameplay"), "countersplat.ui-layout.gameplay.v2");
        assert_eq!(layout_key("selector"), "countersplat.ui-layout.selector.v1");
        let prior = json!({"version":2,"offsets":{"ecs_panel":[100,20],"stats_panel":[-20,2],"weapon_hud":[9,7]},
            "sizes":{"ecs_panel":[500,600],"other_widget":[30,40]}});
        let migrated: Value = serde_json::from_str(&migrate_gameplay_layout(Some(&prior.to_string())).unwrap()).unwrap();
        assert_eq!(migrated, json!({"version":2,"offsets":{"weapon_hud":[9,7]},"sizes":{"other_widget":[30,40]}}));
        let flat: Value = serde_json::from_str(
            &migrate_gameplay_layout(Some(r#"{"scene_panel":[3,4],"weapon_hud":[5,6]}"#)).unwrap(),
        )
        .unwrap();
        assert_eq!(flat, json!({"weapon_hud":[5,6]}));
        for value in [None, Some("invalid"), Some("[]"), Some(r#"{"version":3,"offsets":{},"sizes":{}}"#)] {
            assert_eq!(migrate_gameplay_layout(value), None);
        }
    }

    #[test]
    fn controls_migration_updates_only_the_old_sprint_default() {
        let old = json!({"sprint_multiplier":3,"move_speed":4,"mouse_look":false,"bindings":{"sprint":"ShiftRight"}});
        let mut expected = old.clone();
        expected["sprint_multiplier"] = json!(1.6);
        assert_eq!(read_controls_preference(None, Some(&old.to_string()), Some(1.6)).unwrap(), Some((expected, true)));
        let mut custom = old.clone();
        custom["sprint_multiplier"] = json!(2.2);
        assert_eq!(read_controls_preference(None, Some(&custom.to_string()), Some(1.6)).unwrap(), Some((custom, true)));
        assert_eq!(
            read_controls_preference(Some(&old.to_string()), Some("{}"), Some(1.6)).unwrap(),
            Some((old.clone(), false))
        );
        assert_eq!(read_controls_preference(None, None, Some(1.6)).unwrap(), None);
        assert!(read_controls_preference(None, Some("[]"), Some(1.6)).is_err());
        assert!(read_controls_preference(None, Some(&"x".repeat(16385)), Some(1.6)).is_err());
        assert!(read_controls_preference(Some("{broken"), None, Some(1.6)).is_err());
        let (settings, _) = read_controls_preference(None, Some(&old.to_string()), None).unwrap().unwrap();
        assert_eq!(settings["sprint_multiplier"], 3);
        assert_eq!(GAMEPLAY_EVENT_CHANNELS.len(), 21);
        assert_eq!(player_rig_key("aaaaaaaa"), "countersplat.player-rig.v1.aaaaaaaa");
    }
}
