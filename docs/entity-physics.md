# Entity physics and Lua tuning

The gameplay scene attaches `PhysicsBody` to the `player` entity alongside `PlayerCamera`, `PlayerControls`, and `PlayerRig`.
The body is a kinematic sphere, resolved against the selected level's voxel collider on its worker. Open
**Entity settings** in the canvas to select a live gameplay entity with the arrow buttons, including entities from the scene, level, player and UI layers. **Attach physics** adds a body;
if the entity has no `PlayerCamera` or `Transform`, it also adds a default `Transform`. Physics, collision, and
gravity are independent switches, with sliders for sphere radius, vertical collider offset, and gravity strength.
The player's rig sets its collision sphere below the camera according to eye height and level scale. **Level collision** in the Level assets
panel is a master switch for collision with the level, separate from each body's switches.

The player starts with physics and level collision enabled and gravity disabled, preserving free-fly navigation.
Enable gravity to fall onto level voxels and jump from the ground. With gravity off, Space uses the existing
free-fly jump arc. The player script reads the same ECS component that the panel edits. The collider must be
loaded and enabled before it can block movement; unavailable collision data leaves movement unobstructed.

A scene can configure another body using the same native components:

```json
{
  "Transform": { "position": [0, 2, 0] },
  "PhysicsBody": {
    "enabled": true,
    "collision_enabled": true,
    "gravity_enabled": true,
    "radius": 0.2,
    "collision_offset_y": 0,
    "gravity": 18,
    "velocity": [0, 0, 0],
    "grounded": false
  }
}
```

Declare `Transform` and `PhysicsBody` as native component types in the scene and register the scene ECS adapters.
The browser's `physics_system.lua` calls `engine.physics.step(dt)` once per fixed simulation tick. Scripts can
read or update a body's validated fields through `ecs.get`/`ecs.set`, or request a collision-aware displacement:

```lua
local entities = ecs.get_resource("scene.entities")
local player = entities.player
local body = ecs.get(player, "PhysicsBody")
body.collision_enabled = true
body.gravity_enabled = true
body.radius = 0.3
ecs.set(player, "PhysicsBody", body)
engine.physics.move(player, {0, 0, -1})

ecs.on("physics.collision", function(hit)
    -- hit.entity, hit.attempted, hit.moved, hit.grounded
end)
ecs.on("physics.error", function(error)
    -- error.entity, error.message: an individual collision request failed
end)
```

`engine.physics.move` uses `PlayerCamera.position` when present, otherwise `Transform.position`. Enabled bodies
with collision enabled send movement to the collision worker; disabled bodies or unavailable level collision use
direct movement. Gravity and velocity integration run only for enabled bodies. Physics errors stop the affected
body's queued movement rather than disabling collision for every entity. A game renderer must consume a generic
entity's `Transform` to move that entity's visuals.

Lua can expose existing component **or resource** fields in the selected entity's inspector without copying values:

```lua
engine.export(player, "walk_speed", {
    component = "PlayerControls", field = "move_speed", label = "Walk speed",
    min = 0.1, max = 20
})
engine.export(player, "invert_look", {
    component = "PlayerControls", field = "invert_y", label = "Invert look"
})
ecs.set_resource("game.tuning", {damage = 10})
engine.export(player, "damage", {
    resource = "game.tuning", field = "damage", label = "Damage", min = 0, max = 100
})
engine.set_export(player, "walk_speed", 8)
```

Numbers require finite minimum and maximum bounds that include the current value; booleans need no bounds.
`engine.exports(entity)` lists the metadata, and `engine.unexport(entity, name)` removes a declaration. The panel pages through exports, writes through to the original
component or resource, and applies component validation. Export declarations and edits are runtime scene data;
update scene JSON or script defaults to make them permanent.

The browser currently resolves spherical bodies against the static level voxels. Body-to-body contacts, capsule
shapes, and full rigid-body forces are not yet connected to this browser host. Its generic `Transform` does not
move level splats or UI elements by itself. Native hosts can integrate their physics owner through the same ECS
components and movement events; running the Lua API alone does not create a physics worker.

## Player rig and ground probes

`PlayerRig` stores `eye_height_m`, `radius_m`, `units_per_meter`, and
`auto_align_spawn`. Lua derives the player body's radius and vertical offset from these values:
the camera-to-feet distance is `eye_height_m * units_per_meter`. Movement speed, jump speed,
and gravity are scaled with the rig. The **Player controls** panel exposes eye height and level
scale, while **Entity settings** also exposes them as Lua exports. Editing the player's collision
radius or offset there keeps the rig in sync; other bodies remain independent.

```lua
local player = ecs.get_resource("scene.entities").player
local rig = ecs.get(player, "PlayerRig")
rig.eye_height_m = 1.8
rig.units_per_meter = 1.2
ecs.set(player, "PlayerRig", rig)
```

When voxel data becomes ready, the player script can request a nearby ground height with
`engine.physics.probe_ground(entity, position, radius, max_drop, token)`. The host routes the
asynchronous `physics.ground_result` with the same token and either a numeric `height` or
`ecs.null`. Lua decides whether to use that result. `engine.physics.motion_version(entity)`
lets a script reject results that became stale after movement. The player script preserves
scene-authored spawns and ignores results after movement or when no floor is found.
`physics_system.lua` handles contact velocity and grounded state; the worker supplies geometry
results and the browser host reconciles resolved positions.
