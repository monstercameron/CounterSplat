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
- Shift with movement: sprint. Each direction is its own state with its own
  clip: `runForward` (any forward input, including diagonals), `runBackward`
  (any backward input), `runLeft` and `runRight` (pure strafes). Carries from
  low ready or ADS land in the chosen direction, and changing direction
  mid-sprint switches state with a crossfade.
- Aiming mid-sprint and back, and the low ready ↔ sprint carries, are named
  transition states: `runToADS`, `adsToRun`, `lowToRun`, `runToLow` (low ready
  ↔ ADS stay `raising` / `lowering`).
- C while sprinting: crouch slide (`player_controls.lua`). The body slides along
  the sprint direction. Its speed bursts from sprint speed to a peak of
  `PlayerPosture.slide_speed_multiplier` (1.5) times sprint speed over
  `slide_accel_seconds` (0.15 s, an ease-out), then glides back down to crouch
  speed by `slide_seconds` (1.0 s): it eases off the peak, bleeds speed
  steadily like sliding friction and settles softly into the crouch.
  The body stays crouched for the whole slide even if C is released;
  releasing the movement key that started it (W) or jumping cancels it.
  Afterwards you stay crouched only while C is still held. The weapon follows the
  `player.slide` events into its `slide` state and gets up through
  `slideExit`; aiming or firing leaves it.
- Q (inspection HUD) and I inspect the weapon from an idle low ready:
  `inspectEnter` turns it into the observe pose, `inspecting` holds it and
  `inspectExit` returns. Q holds it for as long as the HUD is open; I for
  `WeaponConfig.inspect_seconds` (2.4 s). Aiming, firing, sprinting or
  reloading cancels it.

The revolver's poses are concept stills (`docs/concepts/revolver/`), and the
moves between them are being filled with Seedance 2.5 footage generated from
each pair of stills (`scripts/seedance_frames.py`, which also keys the frames
and spaces them evenly along the motion). Footage so far:

- low ready → hip → ADS (`raising`);
- the low ready / ADS → sprint carries (`runFromLow`, `runFromADS`);
- the reload, from low ready or ADS: open the cylinder, the filmed reload, then
  close back into the same pose (`reload`, `reloadADS`);
- the inspect turn (`inspect`: low ready → observe pose → back);
- the slide: the drop plays over `slide`'s duration and holds until the slide
  ends, then `slideExit` gets up into low ready (aiming or firing skips it);
- the holster (`holster`): stowing from an idle low ready plays it out of
  frame and equipping plays it backwards. Stowing from any other pose freezes
  that pose and lowers and fades it.

The idle and sprint loops, crouch and the ADS shot still repeat stills. Hip fire has no
shot footage (`fireHip` is disabled: generated takes lunged the barrel out of
the frame), so a hip shot holds the hip pose and kicks it with the spring
recoil. Every shot's view-model kick scales with the weapon's resolved
`WeaponRecoil.vertical_degrees` relative to the 0.55° carbine the springs were
tuned on, so the .500 Magnum (1.84°) kicks about 3.3 times as hard.
`scripts/footage_manifest.py` writes `manifest.json`'s frame lists so poses
change at the right phase; Lua's frame counts are unchanged. Hip shots leave through the camera centre
(`FirstPersonView.aim_from_camera`). Aimed shots leave through the revolver's
front sight as it is drawn: `FirstPersonView.ads_sight` is the fiber-optic dot's
pixel in the 1280x720 ADS frame (639, 306), and the `fireADS` clip's
`sight_anchors` mark the post in each fire frame, so sway, recoil and the
frame's vertical offset move the aim with the sight. To regenerate the frames, convert each still with
`cargo run --release -p countersplat-assets --example footage_still -- in.png out.webp`
and copy it to the path named in the manifest.

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
