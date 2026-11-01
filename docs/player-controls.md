# Player camera and controls

The browser game has a grounded first-person player camera when voxel collision is available. Click the canvas to focus it, then use **W/A/S/D** to move,
**E/Q** to move up/down, **Space** to jump, and **Left Shift** to sprint. In browsers that permit
pointer lock, clicking the scene captures the mouse for continuous look; press **` / ~** to release it. The Codex
in-app browser uses left-drag look when pointer lock is unavailable. Hold **right mouse** for a smooth 5× zoom and release to return to the normal view. **Escape** opens the pause menu. Movement stops on focus loss, pause, or
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

## Camera zoom

[`camera_zoom.lua`](../assets/scripts/camera_zoom.lua) handles right-button zoom with a smooth transition. It computes optical magnification from the tangent of half the field of view, so 5× means five times the focal length at any configured base FOV. The base FOV and player position remain unchanged. Focus loss, pause, and reset release the zoom. **Entity settings → player → Component settings** provides **Zoom magnification** and **Zoom response**; the defaults in `gameplay.json` are 5× and 12 per second.

## Camera bob

[`camera_bob.lua`](../assets/scripts/camera_bob.lua) adds a visual-only sway while the player moves. Open **Entity settings**, select `player`, and use the export-page arrows to tune **View bob**, **Bob steps / sec**, **Bob vertical (m)**, **Bob sideways (m)**, and **Bob response**. The values live in `PlayerControls`, are initialized in `gameplay.json`, and are saved with the other browser control preferences. **Reset controls** restores their scene defaults.

Cadence describes steps per second at normal walk speed and increases with actual travel speed. Amplitudes are in metres and scale with the player rig. Bob starts and settles smoothly, pauses in the air when voxel collision is active, and stops when movement is blocked. The script writes only `PlayerCamera.view_offset`; physics, the collider, spawn position, and streaming distance use the unchanged `PlayerCamera.position`.

The player has a `PhysicsBody` component. Physics, collision and gravity are on by default; the player stands
on the level voxels and can jump while grounded. With gravity off, Space follows a free-fly jump arc.
While voxel data and the initial ground probe load, Lua holds the player in place. If the level has no collider,
gravity is suspended and free-fly navigation remains available. The **Entity settings** panel controls the
body's switches, radius, collider offset and gravity. See [entity physics](entity-physics.md) and
[voxel collision](voxel-collision.md). The player uses an upright kinematic capsule rather than a full rigid-body controller. The native window viewer does not yet host this browser UI or player script. A native game host can register the
same component adapters, instantiate the desired scene layers, dispatch input, and run the Lua system itself.

## Player height and level scale

**Eye height (m)** defaults to 1.7; the rig's collision radius defaults to 0.25 m. The upright
capsule extends 0.15 m above the eyes by default, for a total standing collider height of 1.85 m.
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

The Lua player system derives the capsule radius, vertical half-segment and center offset from
`PlayerRig`, and preserves the player's foot position when height or scale changes. Eye height, scale,
and head clearance are also exported to **Entity settings** for tuning through the same ECS fields.
