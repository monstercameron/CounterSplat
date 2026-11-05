# Physics props and the can range

The Barn (level `58d333ae`) has a can range: a 1 m half wall with ten cans on
top. The cans are rigid bodies, so shooting one knocks it off the wall. Press F
to raise the weapon and T to stand the cans back up.

## How it fits together

`assets/scripts/can_range.lua` decides what exists and where. It places the wall
relative to the player's spawn view, on the floor found by a ground probe. It
gives each can a sphere `Hitbox` so bullets can hit it, and it describes every
body in the `props.layout` resource. When `projectile.impact` hits a can, the
script turns the bullet's velocity into a `prop.impulse` event. T emits
`prop.reset`.

Rust simulates and draws the props:

- `countersplat_physics::prop_runtime` reads the layout and rebuilds the
  simulation whenever its `version` changes. A `null` layout clears it.
- Each frame it applies `prop.reset` and `prop.impulse` events and steps a
  Rapier world.
- It then writes each moving can's position into its `Transform`, so the
  hitboxes follow the cans.
- `countersplat_render::props` draws boxes and cylinders in the scene pass. The
  props write depth, so splats behind them are hidden.

The physics steps at a fixed 1/120 s. Long gaps, such as a hidden tab, are
capped instead of simulated in full.

| Contract | Shape |
| --- | --- |
| `props.layout` resource | `{version, gravity: [x,y,z], bodies: [...]}`, at most 256 bodies |
| body | `shape` (`box` with `half_extents`, or `cylinder` with `radius` and `half_height`), `position`, `yaw`, `dynamic`, `mass`, `friction`, `restitution`, `linear_damping`, `angular_damping`, `color`, `cap_color`, `visible`, and an optional `entity` to bind |
| `prop.impulse` event | `{entity, impulse: [x,y,z], point: [x,y,z]}` in N·s and world units |
| `prop.reset` event | puts every dynamic prop back where the layout placed it, at rest |

## Tuning

The `can_range` resource in `assets/scenes/gameplay.json` holds the settings,
all in metres, kilograms and seconds:

- **Placement:** `forward_m` and `lateral_m`. The wall faces the player.
- **Wall:** length, height, thickness, friction and colour.
- **Cans:** `can_count`. Each can is a 330 ml can: radius 0.033 m, height
  0.122 m, mass 0.015 kg, friction 0.4, restitution 0.25, with light damping.
- **Shots:** `shot_impulse_ns`, the momentum a bullet passing through leaves in
  the can.
- **Level filter:** `level_ids`, the levels the range appears in.

A placement needs a clear line of fire from the spawn, because the barn's
braces have level collision. Bullets stop on that collision even where the
splats look open. To check a candidate placement, run:

```bash
cargo run --release -p countersplat-physics --example floor_probe -- assets/levels/58d333ae 1.925 1.6 -2.422 -2.058 4.5:2.2
```

The example prints, for each `forward:lateral` spot:

- the floor height;
- the free space along the wall;
- whether the line from the eye to each can position is clear.

`apps/threaded-demo/tests/can_range_lua.rs` runs the real scripts and the real
physics end to end. A hip shot at one can knocks only that can off the wall, and
T stands it up again.
