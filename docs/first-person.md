# First-person weapon view

The gameplay scene owns a `first_person` entity with `FirstPersonView`,
`WeaponConfig`, `WeaponAnimation`, and `WeaponState` components. The Lua
controller owns input, ammunition, the state graph, and frame selection. The
inspector exposes visibility, keying, framing, cadence, reload and hip timing,
as well as idle, movement, recoil, and clip-blend controls. Initial magazines
and tuning are defined in the scene JSON.

The Neural-Sight playback frames are local, Git-ignored media under
`assets/images/first-person`; the tracked manifest and source notices describe
the streamable frames. Standing low-ready, crouched idle, and crouched walking use their
authored loops. Aim, sprint and hip-fire use shared carry/raise footage, with
interrupted transitions resuming from the current phase. The accepted sprint
pose is the source's still frame animated by movement sway. The source's ADS
firing clip is disabled, so ADS uses its aim pose with procedural recoil.
Reload, stow/equip, hit reactions, and death each have their own timing and
presentation. Breathing, collision-resolved gait, mouse-follow and spring
recoil layer over the footage; they do not move the player's physics body.

Controls in the gameplay canvas:

- Left mouse: fire. Right mouse: aim and use the camera zoom.
- V: cycle single, three-round burst, and automatic fire.
- Tap R: swap to the fullest spare magazine, retaining the old one. Hold R to
  consolidate rounds between partially filled magazines; release to stop.
- F: stow or equip.

The Lua system emits `weapon.shot` and `weapon.dry_fire` ECS events. Scripts
can request actions through `weapon.trigger` and `weapon.aim` (`pressed`
boolean), `weapon.reload`, `weapon.equip`, `weapon.mode`, `weapon.hit`
(`near` boolean), and `weapon.die`. The posture system emits
`weapon.posture` with a collision-checked `crouch` amount and `moving`
flag. The shot captures the visible sight's camera ray before applying its own
recoil; with the view hidden it uses the camera center. The host publishes
the current render aspect as `render.scene_aspect`.
Fast mouse-driven camera turns produce a directional blur on the weapon layer. The Lua system measures the camera's actual clamped rotation and uses the active standard or zoom lens shutter; the effect fades as the camera settles and is reduced during ADS. The `WeaponMotionBlur` component on **First person** exposes enablement, strength, maximum span, response, and ADS scale in Entity settings. Disabling lens motion blur also disables this effect. It is a keyed, premultiplied compositing effect: it does not move the sight or change gameplay aim, and the world and UI are not sampled by the weapon blur.

Projectile travel, collision, and damage are handled by the separate Lua
system described in [ballistics.md](ballistics.md).

`first_person.frame` carries the chosen clip, frame, direction, visual motion,
and blend timing to the browser host. Its bounded decoded-frame cache resolves
scene-declared assets, prefetches in the current direction, discards stale
asynchronous results, and releases images when the scene exits. Discontinuous
clip changes blend from the last displayed frame. The renderer keys the source
frames and composites them into the live HDR scene before image correction and
UI; it does not interpret weapon states or read ECS data.
