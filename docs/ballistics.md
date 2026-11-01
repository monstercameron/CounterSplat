# Shooting and ballistics

The gameplay scene loads `assets/scripts/ballistics.lua` after the first-person
weapon controller. The controller keeps its firing modes, magazine accounting,
reload timing and input handling; when a round fires, it snapshots the rendered
eye and direction into `weapon.shot`. Later camera movement cannot redirect it.
Lua creates an ECS entity with `Transform` and `Projectile`, and advances it on
fixed simulation ticks using its initial speed and gravity. The selected level's
`PlayerRig.units_per_meter` converts the settings in metres to scene units.

Select **First person** in the ECS entity panel and page through its exported
component settings to tune bullet speed, gravity, maximum range, lifetime, damage
and trace visibility. Defaults live in the scene's `Ballistics` component; a
shot retains its settings from the moment it was fired. Traces and brief impact
sparks are rendered on the WebGPU canvas. The HUD reports impact distance and
simulated travel time. Worker latency can delay when a result appears, but it
never moves the projectile past an unchecked segment or makes an impact happen
before its simulated travel time.

The collision worker clips each segment to the level grid and traverses the
sparse voxel octree cell by cell. It returns the first hit fraction, position
and normal in world coordinates, including for legacy-coordinate levels. Lua
compares that hit with the nearest eligible sphere `Hitbox` and handles the
first impact. Results carry a projectile entity and unique token; late or
repeated replies are ignored. Failed or timed-out queries end the projectile
without treating the unchecked path as clear. Shots wait for a loading collider,
with a bounded timeout. Disabling level physics or using a level without voxel
data leaves level geometry uncollidable; the HUD reports that condition. Entity
hitboxes continue to work.

To make an entity shootable, give it a `Transform` (or `PlayerCamera`) and a
dynamic component such as:

```json
"Hitbox": { "enabled": true, "radius": 0.5, "offset": [0, 1, 0] },
"Health": { "current": 100 }
```

The spherical radius and offset are in world units. A `Health` component is
optional; when present, Lua subtracts the configured damage and emits
`combat.damage`, followed by `combat.death` when health first reaches zero.
`projectile.spawned`, `projectile.impact` and `projectile.expired` are ordinary
ECS events. Impact data includes the target or level, source, position, normal,
shot number, distance and travel time. The shooter and weapon are excluded from
hitbox tests. This first implementation does not simulate penetration,
ricochets, drag or moving-target swept volumes.

For other scripts, `engine.physics.trace_segment(entity, start, finish, token)`
submits a finite world-space query. The browser collision owner responds with a
`physics.trace_result` event whose status is `hit`, `miss`, `unavailable` or
`error`; a hit contains `fraction`, `position` and `normal`. The query itself
contains no weapon or damage behavior. Hosts that use this API must provide a
collision owner and forward its result events to their Lua scene.
