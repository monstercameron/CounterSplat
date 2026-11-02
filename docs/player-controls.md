# Player camera and controls

The browser game has a grounded first-person player camera when voxel collision is available. Click the canvas to focus it, then use **W/A/S/D** to move,
**E/Q** to move up/down, **Space** to jump, hold **Shift** to run, and hold **Ctrl** to crouch. In browsers that permit
pointer lock, clicking the scene captures the mouse for continuous look; press **` / ~** to release it. The Codex
in-app browser uses left-drag look when pointer lock is unavailable. Hold **right mouse** to aim using the equipped optic’s magnification; release to return to the normal view. **Escape** opens the pause menu. Movement stops on focus loss, pause, or
when UI arrangement starts, so a missed key release cannot leave the player moving.

Open **Player controls** in the lower right of the canvas to change movement and mouse-look enablement, speed,
sprint multiplier, look sensitivity, field of view, jump speed and gravity, eye height, level scale, Y inversion, and all nine key bindings. Click a binding and
press the desired physical key; Escape cancels a pending binding. **Reset controls** restores the scene defaults.
**Reset view** in the Scene view panel returns the camera to the starting position and orientation while retaining
a chosen FOV. Browser-local preferences survive scene changes and refreshes and do not rewrite scene JSON.

`PlayerCamera`, `PlayerControls`, `PlayerRig`, and `CameraController` are validated, serializable components in `countersplat-scene`; their generic
ECS adapters live in `countersplat-scene-ecs`. The gameplay manifest creates a `player` entity in the `player`
layer. The browser instantiates the `player` and `ui` layers into one gameplay ECS world, so Lua can use the same
`ecs.get` and `ecs.set` APIs for camera state and UI controls. The skybox, level, player and UI layers share the gameplay ECS world. The
renderer reads the current camera component and uses it for both the view matrix and streaming selection.

By default, `resources.preview.player_spawn` is `"level"`, which seeds the camera from the selected level's camera
hint (or the scan bounds when no hint exists). Set it to `"scene"` to use the `PlayerCamera` pose in gameplay JSON.
The host publishes a validated candidate or an error as the `player.level_view` ECS resource;
`player_controls.lua` selects the spawn, updates `PlayerCamera`, and records `player.spawn_camera`
for reset. An invalid level candidate is ignored when the scene pose is selected. Other hosts can omit the
candidate and supply a reset pose, or let the script use the authored camera. Lua also decides whether to
adjust the spawn to a collision-probed floor.
Yaw is radians with zero facing -Z; positive pitch looks upward. `PlayerControls` stores movement speed in
nominal meters per second, sensitivity in radians per pointer pixel, and physical key codes such as `KeyW` or `ArrowUp`.
An optional `fov_degrees` setting overrides the spawn camera's FOV. Scene defaults are validated on loading and
script writes use the same validation; malformed saved browser preferences are ignored.

[`player_controls.lua`](../assets/scripts/player_controls.lua) handles movement, jumping, control settings and
key bindings. [`neural_camera.lua`](../assets/scripts/neural_camera.lua) owns direct mouse look and turn tilt
when the `camera_controller` entity is enabled. It resolves its `target` scene ID to the current player camera.
The browser distributes input events to registered Lua listeners and runs systems at the fixed simulation tick.
The Lua systems own player and weapon behavior; Rust supplies validated ECS components, collision calculations,
and rendering. The browser collision worker transports asynchronous query results back to Lua for gameplay
decisions.
If the controller is disabled or removed, the original direct-look handler remains available; the existing
`PlayerControls.mouse_look` switch still enables or disables mouse look. The navigation script receives the same
events, preserving Escape menu behavior. `ecs.on` events are host-dispatched copies and are distinct from the
single-consumer ECS queues.


## Running and crouching

Either Shift key runs while held. The gameplay scene defaults to 4 m/s walking and a 1.6× sprint (6.4 m/s at the default rig scale); both remain configurable in **Player controls**. Either Ctrl key crouches while held; the
bindings can be changed in **Player controls**. Crouch takes priority over run
and uses the scene's crouch-speed multiplier. The player entity's
`PlayerPosture` component exposes crouch enablement, height ratio, movement
speed, and transition time through **Entity settings**.

[`player_posture.lua`](../assets/scripts/player_posture.lua) owns the behavior.
It shortens the player's actual capsule and preserves its foot position while
the camera eases to the new eye height. Releasing Ctrl checks the added head
volume on the voxel worker before restoring standing height. If a ceiling is in
the way, the player remains crouched and can move out from under it. If the
collider is disabled or unavailable, the player can stand without that query.
Changes to rig scale or eye height made while crouched apply when standing is
clear. Crouch posture and movement are sent to the first-person footage script.

## NeuralSight camera controller

The default `camera_controller` entity ports the direct look and spring-smoothed turn tilt from Neural-Sight's
`app/src/main.js` and `app/src/turn-tilt.js`. Its scene default mouse sensitivity is 0.11 degrees per pointer
pixel; the existing sensitivity slider uses radians per pixel, and any saved user preference takes precedence.
Pitch is limited to 80 degrees by default. Camera movement is immediate; the small visual roll follows the
turn rate, settles smoothly, and is reduced while the right mouse button is held.

Open **Camera & lens** for the turn-tilt switch, strength slider, and **Reset camera motion**. Select
**Camera controller** in **ECS entities** to tune the pitch limit, maximum tilt, response speeds, aim damping,
and gait rotation. The component and its target live in scene JSON; inspector and panel changes are saved
locally in the browser, and reset returns to the scene defaults. The camera controller can be retargeted by
setting its `target` to another scene entity with `PlayerCamera` and `PlayerControls`; the target remains
scene-owned when browser tuning is restored.

`PlayerCamera.view_turn_roll`, `view_bob_pitch`, and `view_bob_roll` are visual offsets. They are composed
with the authored camera orientation when rendering and do not change the player's base aim, body, or collision
position. Reset, pointer-lock loss, pause, and removing the controller clear transient turn motion.

## Camera zoom

[`camera_zoom.lua`](../assets/scripts/camera_zoom.lua) handles right-button or scripted ADS with a smooth transition. Iron sights use 1.25×, red dots use 1.5×, and scopes use the equipped `WeaponOptic.scope_magnification`. Magnification is calculated from the tangent of half the field of view, keeping the base FOV and player position unchanged. Focus loss, pause, and reset release ADS. **Camera & lens → Optic** selects the sight; **Edit zoom lens** exposes scope magnification for scopes. Zoom response remains available in the player entity settings. The old `PlayerControls.zoom_factor` field is retained for saved-data compatibility but no longer determines weapon ADS.

## Camera bob

[`camera_bob.lua`](../assets/scripts/camera_bob.lua) adds a visual-only sway while the player moves. Open **Entity settings**, select `player`, and use the export-page arrows to tune **View bob**, **Bob steps / sec**, **Bob vertical (m)**, **Bob sideways (m)**, and **Bob response**. The values live in `PlayerControls`, are initialized in `gameplay.json`, and are saved with the other browser control preferences. **Reset controls** restores their scene defaults.

Cadence describes steps per second at normal walk speed and increases with actual travel speed. The active
NeuralSight profile adapts `app/src/view-bob.js`: longer running strides, an asymmetric running foot plant,
small visual pitch and roll, and reduced gait while aiming. Positional amplitudes remain configurable in metres
and scale with the player rig. Bob starts and settles smoothly, pauses in the air when voxel collision is active,
and stops when movement is blocked. `PlayerCamera.position`, gameplay aim, collision, and the spawn pose stay
unchanged by these visual offsets. Collision-resolved speed is smoothed so asynchronous collision batches do not make the bob jump or flicker. Older scenes without the controller retain the original bob behavior.

The optional player `SprintVisuals` component provides inspector exports for sprint bob multipliers, peripheral blur enablement, strength, and response. Lua drives the blur from actual resolved movement, fading it out on stopping, blocking, crouching, aiming, or leaving the ground. It remains available when view bob is disabled. Existing v1 browser controls with the former 3× sprint default migrate once to the scene default; custom sprint values and other settings are preserved.

The player also owns a `CombatVision` component. Its Lua system listens for that player's actual `weapon.shot` events: frequent shots gradually build a restrained tunnel effect, while isolated shots stay below the default threshold. It decays smoothly after firing and does not change the camera aim or projectile path. In **Entity settings**, select **Player** to adjust the effect's enablement, maximum strength, buildup per shot, threshold, recovery time, and response. It works independently of sprint blur, including while aiming; pausing or losing input focus clears it.

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

The **Camera & lens** panel adds field of view, yaw, pitch, roll, photographic exposure, shutter and focus controls.
See [camera and lens](camera.md) for the ECS component, Lua API, and limits of the real-time blur effects.


## Aim stability

The first-person weapon uses the Lua-owned AimStability component for an angular sway envelope. Collision-resolved walking increases sway and recoil; crouching reduces both. ADS eases into its own multiplier. Burst and automatic fire build instability at separately configurable rates, and it recovers smoothly after firing stops. Existing recoil and viewmodel animation settings remain independent.

Shots follow the same sight movement shown by the first-person layer, including the current zoom and viewport aspect. No separate random spread is added behind the visible sight. Open the ECS HUD with the backquote key, select **First person**, and use the Entity settings pages to tune the AimStability fields. The player's SuppressionReceiver settings enable near-miss detection and set its radius in metres from the player's capsule. AimStabilityState reports the current movement, crouch, ADS, firing heat, incoming pressure, and angular sway for engine scripts.

Hostile projectiles produce one combat.near_miss event after passing close to the player, using the confirmed unobstructed portion of their path. A direct hit instead sends combat.damage. Both feed the aim stability model; the player's own shots do not. Asynchronous collision checks retain the player pose from the submitted projectile segment. There are no hostile actors in the dojo yet, so the incoming-fire response requires a hostile projectile or a scripted event.

Other Lua systems can emit `ecs.emit('combat.suppression', {target=player, owner=attacker, intensity=0.7})` to apply incoming pressure. Intensity is clamped to 0–1. `engine.aim_stability.new(weapon, player)` creates an independent model for another weapon with AimStability and SuppressionReceiver components.

### Muzzle rise

The first-person entity's `WeaponRecoil` component controls actual camera aim:
vertical rise and horizontal kick in degrees per shot, hip and ADS multipliers,
and the time over which the kick is applied. Select **First person** in the ECS
entity inspector to tune or disable these values. The existing `WeaponAnimation`
recoil controls only the filmed weapon's motion.

The Lua controller snapshots each shot's visible aim before applying its own
kick. Camera rise accumulates during a burst and remains after firing stops;
pulling the mouse down counters it. The same walking, crouching, and sustained
fire modifiers used by aim stability affect camera recoil. The kick respects
the camera's pitch limit and does not reset mouse corrections.

For filmed weapon clips, optional `sight_anchors` in the scene's
`first_person_clips` describe each frame's sight point in source-image pixels;
`sight_frame_size` supplies that source resolution. The browser reports the frame actually submitted to the canvas, and Lua projects
its visible sight to produce the next shot direction. Hosts without frame
presentation feedback use the latest Lua pose. Clip blending interpolates the
two sight anchors; camera recoil and projectile travel remain independent.

### Running and firing

Holding Shift while moving keeps the player at sprint speed during hip fire.
The Lua weapon controller briefly transitions from the run carry into the
hip-fire pose, fires single, burst, or automatic shots, then returns to the run
carry when firing ends. The first-person entity's `WeaponConfig` exposes
`sprint_fire_exit_seconds` and `sprint_fire_raise_seconds` for tuning this
transition. A new shot reverses a lowering weapon without waiting for the full
lowering animation. ADS still takes priority over sprint; crouch prevents sprint.
Stowing cancels an in-progress reload without swapping the magazine, and input
reset clears queued reloads and trigger requests. Holding aim with the weapon
stowed does not block sprint; if aim is still held when the weapon is equipped,
the weapon and camera return to ADS together.
