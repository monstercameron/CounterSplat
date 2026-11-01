# Scene JSON

The level selector is [`level-selector.json`](../assets/scenes/level-selector.json), gameplay is
[`gameplay.json`](../assets/scenes/gameplay.json), and the pause menu is [`menu.json`](../assets/scenes/menu.json)
in the project assets folder. It contains the ECS component
values for the scene and UI layers, asset and script catalogs, and named application resources. The browser preview
reads this file on refresh; edits to its declared local resources are served directly by the local preview server.
The browser build also packages a copy for a static deployment. Rebuild the browser package after changing Rust or
when preparing that packaged copy, not for ordinary JSON edits with the local server.

A small scene has this shape (see the project file for the complete UI):

```json
{
  "version": 1,
  "name": "Example",
  "component_types": {
    "Skybox": { "storage": "native" },
    "Position": { "storage": "dynamic" }
  },
  "resources": {
    "assets": { "evening": { "kind": "radiance_hdr", "uri": "../images/evening.hdr", "load": "preload" } },
    "scripts": { "motion": { "language": "lua", "uri": "../scripts/motion.lua", "autoload": false, "load": "streamable" } },
    "game": { "gravity": 9.8 }
  },
  "entities": [
    { "id": "sky", "layer": "scene", "components": { "Skybox": { "asset": "evening" } } },
    { "id": "player", "layer": "scene", "components": { "Position": { "x": 0, "y": 1 } } }
  ]
}
```

`component_types` declares each component used by an entity. `native` requires the host to register the component's
validated ECS adapter; `dynamic` stores arbitrary JSON through the ECS dynamic component API. An unknown or mismatched
component declaration fails the load. JSON values for native components follow the type's serde representation and
validation rules. The built-in `Skybox`, `UiElement`, and `FrameCounter` types have adapters; games can register their
own. Native type implementations and system behavior remain Rust code, while the scene owns their initial values.

Entity IDs are stable names within the manifest, not runtime ECS handles. Each entity specifies a layer and optional
`parent` ID. IDs must be unique, parents must exist in the same layer, and cycles are rejected. The preview instantiates
the skybox `scene` layer separately, while its `player` and `ui` layers share a gameplay ECS world and runtime
handles. The application resource `preview` binds specific IDs to this example's camera, sky, FPS, and UI behavior. Its UI controls address the
skybox ID named by `preview.skybox_entity`; general scene extraction still selects enabled skyboxes by priority. The preview's panel `UiElement` values set
`collapsible` headers and initial collapsed state; see [UI panels](ui.md#collapsible-panels).

Assets have a host-defined `kind` and a `uri`; relative paths resolve from the scene file. Asset and script `load` policies, the loading component, and level-transition behavior are described in [asset loading](asset-loading.md). Browser levels and their
validated catalog live under `assets/levels`; the selector loads the chosen scan before starting gameplay. Catalog entries
default to a 180-degree Z rotation for exported SuperSplat coordinates; set `rotate_z_180` to `false` for a scan already
stored in viewer coordinates. Images and
scripts are stored under `assets/images` and `assets/scripts`, with `assets/sound` and `assets/music` reserved for audio.
Browser and native examples load up to eight
referenced HDR sky assets. The native `window-level` command uses the project `assets/scenes/gameplay.json` by default, accepts
`--scene=path/to/scene.json`, and still accepts explicit sky options as overrides. Its positional level directory
selects the native scan. The browser example accepts Radiance `.hdr` skies and runs their decoding on a worker.

Script entries are catalogs, not embedded source. [`rotate_sky.lua`](../assets/scripts/rotate_sky.lua) demonstrates
using `scene.entities` and a scene component. A native Lua-capable host can execute it through
`countersplat-scripting-lua`. Other resources such as `game` become dynamic ECS resources. `scene.assets`,
`scene.scripts`, and `scene.entities` are reserved, script-readable resources published by the loader;
`scene.entities` maps IDs in the instantiated layer to world-scoped handles.

The browser preview runs Lua 5.4 using Wasmoon. It loads scripts marked `autoload`, plus the script IDs in
`resources.preview.browser_scripts` for the browser's gameplay UI layer. Each running scene has its own Lua VM and
ECS world. [`gameplay_navigation.lua`](../assets/scripts/gameplay_navigation.lua) receives `input.key` and
`ui.action` events to open the menu or select levels; [`player_controls.lua`](../assets/scripts/player_controls.lua)
uses the same events and fixed ticks to control the player camera. [`menu_navigation.lua`](../assets/scripts/menu_navigation.lua)
handles Escape, Resume, and Choose level. The browser bridge exposes the gameplay player/UI world to Lua; the
separate gameplay skybox world remains independent. See [player controls](player-controls.md) for input and camera configuration. The native window example has no Lua UI
host, so the gameplay browser entry script is explicitly listed rather than marked for native autoload.

`engine.scene.push(id)`, `engine.scene.pop()`, and `engine.scene.replace(id)` emit requests on the `engine.scene`
ECS queue. IDs refer to scene assets declared in the active manifest, not arbitrary paths. The host validates and
applies the request after the Lua system pass. The browser preview currently supports pushing its pause menu,
popping back to the retained gameplay scene, and replacing either with the level selector. It pauses simulation
while the menu is active, continues drawing into the canvas, and resumes without reloading the level. Other hosts
can implement additional scene routes using the same event contract. UI action queues are single-consumer; a host
that needs multiple listeners must distribute copies. The browser gameplay host drains the shared input and UI
queues and dispatches copies to registered `ecs.on` listeners before fixed-tick systems run.

## Host API

1. Read bytes using the host's file or network API and call `SceneManifest::from_json`. It checks the version, size,
   declarations, IDs, references, and hierarchy without loading assets or executing scripts.
2. Create a `World`, register the native adapters needed by the layer, then call
   `countersplat_scene_ecs::instantiate_layer(&mut world, &manifest, "scene")`. The returned map relates manifest IDs
   to ECS handles. Component adapters validate their values. Failed component or hierarchy writes remove the entities
   created by that call; resources are published only on success, and existing resources are not overwritten.
3. The host resolves asset IDs, loads resource bytes on the appropriate worker, and gives decoded resources to their
   owning subsystem. The host decides when and where to load scripts. A scene manifest never executes code by itself.

The JSON format is versioned and limits input size to 4 MiB. It is initialization data; runtime ECS or UI edits do not
write back to the file. Save changes in `assets/scenes/gameplay.json` and refresh the local browser preview to apply them.
