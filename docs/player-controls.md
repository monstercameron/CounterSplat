# Player camera and controls

The browser game has a free-flying player camera. Click the canvas to focus it, then use **W/A/S/D** to move,
**E/Q** to move up/down, and **Left Shift** to sprint. Drag the scene to look around. In browsers that permit
pointer lock, clicking the scene also captures the mouse for continuous look; the Codex in-app browser uses drag
look when pointer lock is unavailable. **Escape** opens the pause menu. Movement stops on focus loss, pause, or
when UI arrangement starts, so a missed key release cannot leave the player moving.

Open **Player controls** in the lower right of the canvas to change movement and mouse-look enablement, speed,
sprint multiplier, look sensitivity, field of view, Y inversion, and all seven key bindings. Click a binding and
press the desired physical key; Escape cancels a pending binding. **Reset controls** restores the scene defaults.
**Reset view** in the Scene view panel returns the camera to the starting position and orientation while retaining
a chosen FOV. Browser-local preferences survive scene changes and refreshes and do not rewrite scene JSON.

`PlayerCamera` and `PlayerControls` are validated, serializable components in `countersplat-scene`; their generic
ECS adapters live in `countersplat-scene-ecs`. The gameplay manifest creates a `player` entity in the `player`
layer. The browser instantiates the `player` and `ui` layers into one gameplay ECS world, so Lua can use the same
`ecs.get` and `ecs.set` APIs for camera state and UI controls. The skybox stays in its own scene world. The
renderer reads the current camera component and uses it for both the view matrix and streaming selection.

By default, `resources.preview.player_spawn` is `"level"`, which seeds the camera from the selected level's camera
hint (or the scan bounds when no hint exists). Set it to `"scene"` to use the `PlayerCamera` pose in gameplay JSON.
Yaw is radians with zero facing -Z; positive pitch looks upward. `PlayerControls` stores movement speed in world
units per second, sensitivity in radians per pointer pixel, and physical key codes such as `KeyW` or `ArrowUp`.
An optional `fov_degrees` setting overrides the spawn camera's FOV. Scene defaults are validated on loading and
script writes use the same validation; malformed saved browser preferences are ignored.

[`player_controls.lua`](../assets/scripts/player_controls.lua) consumes `input.key`, `input.mouse_delta`,
`input.reset`, and `ui.action` via `ecs.on`. The browser distributes each event to the registered listeners and
runs Lua systems at the engine's fixed simulation tick. The script normalizes diagonal movement, clamps pitch,
tracks key releases, applies UI changes through ECS, and emits `player.controls_changed` for browser persistence.
The navigation script receives the same key and UI events, preserving Escape menu behavior. `ecs.on` events are
host-dispatched copies; they are distinct from the single-consumer ECS queues.

This is a camera controller, not a physics character controller: it does not yet apply gravity or collision.
The native window viewer does not yet host this browser UI or player script. A native game host can register the
same component adapters, instantiate the desired scene layers, dispatch input, and run the Lua system itself.
