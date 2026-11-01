# Player camera and controls

The browser game has a free-flying player camera. Click the canvas to focus it, then use **W/A/S/D** to move,
**E/Q** to move up/down, **Space** to jump, and **Left Shift** to sprint. In browsers that permit
pointer lock, clicking the scene captures the mouse for continuous look; press **` / ~** to release it. The Codex
in-app browser uses drag look when pointer lock is unavailable. **Escape** opens the pause menu. Movement stops on focus loss, pause, or
when UI arrangement starts, so a missed key release cannot leave the player moving.

Open **Player controls** in the lower right of the canvas to change movement and mouse-look enablement, speed,
sprint multiplier, look sensitivity, field of view, jump speed and gravity, eye height, level scale, Y inversion, and all eight key bindings. Click a binding and
press the desired physical key; Escape cancels a pending binding. **Reset controls** restores the scene defaults.
**Reset view** in the Scene view panel returns the camera to the starting position and orientation while retaining
a chosen FOV. Browser-local preferences survive scene changes and refreshes and do not rewrite scene JSON.

`PlayerCamera`, `PlayerControls`, and `PlayerRig` are validated, serializable components in `countersplat-scene`; their generic
ECS adapters live in `countersplat-scene-ecs`. The gameplay manifest creates a `player` entity in the `player`
layer. The browser instantiates the `player` and `ui` layers into one gameplay ECS world, so Lua can use the same
`ecs.get` and `ecs.set` APIs for camera state and UI controls. The skybox, level, player and UI layers share the gameplay ECS world. The
renderer reads the current camera component and uses it for both the view matrix and streaming selection.

By default, `resources.preview.player_spawn` is `"level"`, which seeds the camera from the selected level's camera
hint (or the scan bounds when no hint exists). Set it to `"scene"` to use the `PlayerCamera` pose in gameplay JSON.
Yaw is radians with zero facing -Z; positive pitch looks upward. `PlayerControls` stores movement speed in
nominal meters per second, sensitivity in radians per pointer pixel, and physical key codes such as `KeyW` or `ArrowUp`.
An optional `fov_degrees` setting overrides the spawn camera's FOV. Scene defaults are validated on loading and
script writes use the same validation; malformed saved browser preferences are ignored.

[`player_controls.lua`](../assets/scripts/player_controls.lua) consumes `input.key`, `input.mouse_delta`,
`input.reset`, and `ui.action` via `ecs.on`. The browser distributes each event to the registered listeners and
runs Lua systems at the engine's fixed simulation tick. The script normalizes diagonal movement, clamps pitch,
tracks key releases, applies UI changes through ECS, and emits `player.controls_changed` for browser persistence.
The navigation script receives the same key and UI events, preserving Escape menu behavior. `ecs.on` events are
host-dispatched copies; they are distinct from the single-consumer ECS queues.

The player has a `PhysicsBody` component. Physics and collision are on by default, while gravity is off to keep
free-fly navigation. With gravity off, jump follows a time-based arc; with gravity on, the player falls onto the
level collider and can jump while grounded. The **Entity settings** panel controls the body's switches, radius,
collider offset and gravity. See [entity physics](entity-physics.md) and [voxel collision](voxel-collision.md).
The browser uses a kinematic sphere rather than a full rigid-body character controller. The native window viewer does not yet host this browser UI or player script. A native game host can register the
same component adapters, instantiate the desired scene layers, dispatch input, and run the Lua system itself.

## Player height and level scale

**Eye height (m)** defaults to 1.7; the rig's collision radius defaults to 0.25 m.
**Scale vs level** controls how many scan units represent one meter, defaulting to 1.0.
Scans do not necessarily contain a reliable physical scale, so tune this value against familiar
features in each level. Increasing it makes the player, movement, jumps, and gravity larger in
scan units without rescaling the scan. The browser saves these rig settings separately for each
level. **Reset controls** restores the scene or catalog defaults for the current level.

When an imported level spawn has nearby voxel floor data, `player_controls.lua` requests a floor
probe and positions the eye at the chosen height. It also updates the reset position. If no suitable
floor is found, the collider is unavailable, the player has moved, or the scene explicitly uses
`preview.player_spawn = "scene"`, the imported or authored pose is retained. Set
`PlayerRig.auto_align_spawn` to false to disable this adjustment. An optional catalog
`player_units_per_meter` sets a level-specific initial scale; saved per-level settings take precedence.

The Lua player system derives the collision radius and offset from `PlayerRig` and preserves the
player's foot position when height or scale changes. The eye height and scale are also exported to
**Entity settings** for tuning through the same ECS fields.
