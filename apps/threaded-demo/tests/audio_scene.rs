//! Real scene -> Lua/ECS edits -> owned command -> audio owner thread -> stereo samples.
use countersplat_audio::{
    AudioClip, AudioCommand, AudioConfig, AudioReply, OfflineMixer, StereoFrame,
};
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_runtime::native::Worker;
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use std::{cell::RefCell, rc::Rc, time::Duration};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
enum Command {
    Audio(AudioCommand),
    Render(usize),
}
enum Reply {
    Audio(AudioReply),
    Frames(Vec<StereoFrame>),
}

#[test]
fn audio_scene_and_lua_drive_a_separate_audio_owner() {
    let json = br#"{
      "version":1,"name":"Audio example",
      "component_types":{"AudioLayer":{"storage":"native"},"AudioEmitter":{"storage":"native"}},
      "resources":{"assets":{"step":{"kind":"sound","uri":"../sound/step.wav"}}},
      "entities":[
        {"id":"effects","layer":"audio","components":{"AudioLayer":{"name":"sound"}}},
        {"id":"emitter","layer":"audio","components":{"AudioEmitter":{"asset":"step","layer":"sound","autoplay":true}}}
      ]
    }"#;
    let scene = SceneManifest::from_json(json).unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_audio_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::instantiate_layer(&mut world.borrow_mut(), &scene, "audio").unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        r#"
      local ids = ecs.get_resource("scene.entities")
      local emitter = ecs.get(ids.emitter, "AudioEmitter")
      emitter.settings.balance = -1
      emitter.settings.looped = true
      ecs.set(ids.emitter, "AudioEmitter", emitter)
      emitter.settings.balance = 2
      assert(not pcall(function() ecs.set(ids.emitter, "AudioEmitter", emitter) end))
      local layer = ecs.get(ids.effects, "AudioLayer")
      layer.mix.gain = 0.5
      ecs.set(ids.effects, "AudioLayer", layer)
    "#,
    )
    .unwrap();
    let snapshot = countersplat_audio_ecs::extract(&world.borrow());
    assert_eq!(snapshot.layers.len(), 1);
    assert_eq!(snapshot.emitters.len(), 1);
    let layer = &snapshot.layers[0].1;
    let emitter = &snapshot.emitters[0].1;
    assert_eq!(emitter.settings.balance, -1.0);
    assert!(emitter.autoplay);
    let mut worker = Worker::spawn(
        "audio",
        8,
        || {
            OfflineMixer::offline(
                AudioConfig {
                    transition: Duration::ZERO,
                    ..Default::default()
                },
                48_000,
            )
            .map_err(|e| e.to_string())
        },
        |mixer: &mut OfflineMixer, command| match command {
            Command::Audio(cmd) => mixer
                .apply(cmd)
                .map(Reply::Audio)
                .map_err(|e| e.to_string()),
            Command::Render(frames) => mixer
                .render(frames)
                .map(Reply::Frames)
                .map_err(|e| e.to_string()),
        },
    )
    .unwrap();
    worker.wait_ready(Duration::from_secs(2)).unwrap();
    let client = worker.client();
    assert!(matches!(
        client
            .try_submit(Command::Audio(AudioCommand::SetLayer {
                name: layer.name.clone(),
                mix: layer.mix
            }))
            .ok()
            .unwrap()
            .wait()
            .unwrap(),
        Reply::Audio(AudioReply::Applied)
    ));
    let clip = AudioClip::from_pcm(48_000, 1, &vec![0.2; 4096]).unwrap();
    assert!(matches!(
        client
            .try_submit(Command::Audio(AudioCommand::Play {
                layer: emitter.layer.clone(),
                clip,
                settings: emitter.settings
            }))
            .ok()
            .unwrap()
            .wait()
            .unwrap(),
        Reply::Audio(AudioReply::Voice(_))
    ));
    let frames = match client
        .try_submit(Command::Render(512))
        .ok()
        .unwrap()
        .wait()
        .unwrap()
    {
        Reply::Frames(frames) => frames,
        _ => panic!("wrong audio reply"),
    };
    let frame = frames.last().unwrap();
    assert!((frame.left - 0.1 * 2.0_f32.sqrt()).abs() < 0.015);
    assert!(frame.right.abs() < 0.001);
    worker.shutdown().unwrap();
}
#[test]
fn gameplay_lua_plays_scene_emitters_for_weapon_and_footsteps() {
    use serde_json::json;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_audio_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &scene,
        &["ui", "player", "audio"],
    )
    .unwrap();
    let player = ids["player"].to_string();
    let weapon = ids["first_person"].to_string();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/game_audio.lua")).unwrap())
        .unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("audio.layer")[0]["name"],
        "sound"
    );
    for (name, value, asset) in [
        ("weapon.shot", json!({"player":player}), "sound-rifle-shot"),
        (
            "weapon.reload_started",
            json!({"entity":weapon}),
            "sound-rifle-reload",
        ),
        (
            "weapon.dry_fire",
            json!({"entity":weapon}),
            "sound-dry-fire",
        ),
        (
            "projectile.impact",
            json!({"owner":player}),
            "sound-bullet-impact",
        ),
    ] {
        lua.dispatch(name, &value).unwrap();
        assert_eq!(
            world.borrow_mut().drain_events("audio.play")[0]["asset"],
            asset
        );
    }
    world
        .borrow_mut()
        .get_mut::<countersplat_scene::PhysicsBody>(ids["player"])
        .unwrap()
        .grounded = true;
    lua.dispatch(
        "physics.motion_resolved",
        &json!({"entity":player,"moved":[0,0,1.5]}),
    )
    .unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("audio.play")[0]["asset"],
        "sound-footstep"
    );
    lua.dispatch("weapon.shot", &json!({"player":"other"}))
        .unwrap();
    assert!(world.borrow_mut().drain_events("audio.play").is_empty());
}

#[test]
fn pause_requests_update_the_scene_audio_layer() {
    use serde_json::json;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_audio_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &scene,
        &["ui", "player", "audio"],
    )
    .unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/game_audio.lua")).unwrap())
        .unwrap();
    world.borrow_mut().drain_events("audio.layer");
    lua.dispatch("settings.sound_request", &json!({"gain":0.35}))
        .unwrap();
    let layer = world
        .borrow_mut()
        .drain_events("audio.layer")
        .pop()
        .unwrap();
    assert!((layer["mix"]["gain"].as_f64().unwrap() - 0.35).abs() < 0.0001);
    lua.dispatch("settings.sound_request", &json!({"muted":true}))
        .unwrap();
    let layer = world
        .borrow_mut()
        .drain_events("audio.layer")
        .pop()
        .unwrap();
    assert_eq!(layer["mix"]["muted"], true);
    lua.execute("local a=ecs.get(ecs.get_resource('scene.entities').audio_root, 'AudioLayer'); assert(a.mix.muted and math.abs(a.mix.gain-.35)<.0001)").unwrap();
}
