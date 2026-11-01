# Level voxel collision

The browser loads PlayCanvas / SuperSplat sparse voxel data from the selected level's
`assets/levels/<level_id>/scene.voxel.json` and `scene.voxel.bin`. The binary and metadata stay in a dedicated
Web Worker, which checks the octree and answers movement and debug-view requests. The renderer draws the debug
lines directly on the WebGPU canvas. See the [PlayCanvas voxel format](https://developer.playcanvas.com/user-manual/splat-transform/voxel-format/)
and [SuperSplat Viewer](https://github.com/playcanvas/supersplat-viewer/blob/main/src/collision/voxel-collision.ts)
for the upstream format and collision approach. The format's legacy 1.0 coordinate convention and 1.1 world-space
convention are both supported; missing version metadata is treated as 1.0.

In the **Level assets** panel:

- **Enable voxels** is the master switch. Turning it off suspends collision and the debug view while retaining
  the other two settings.
- **Show voxels** shows cyan, x-ray wireframe boxes for occupied voxels near the camera. The visualization is
  limited to 6,000 voxels within 3 world units and updates as the player moves; it is a local debug view rather
  than an exhaustive display of the level.
- **Level collision** enables collision with occupied voxels for all enabled entity bodies, independently of visibility.

Each entity's `PhysicsBody` independently controls whether its kinematic sphere collides with the level,
its radius, and its vertical offset from the camera or transform. The player can optionally use gravity and
jump from the floor; with gravity off it remains free-flying. Movement uses bounded substeps and slides along
obstacles. The browser currently resolves bodies against level voxels, not against each other. Physics becomes
active once the worker has loaded the level's voxel files. Until then, movement remains unobstructed. A level
with no voxel metadata remains playable and reports that no collider is available; malformed or missing binary
data reports an unavailable collider. The worker limits metadata to 1 MiB and voxel binaries to 128 MiB.

The `active_level` entity in [`gameplay.json`](../assets/scenes/gameplay.json) has the validated `VoxelCollider`
component and the host-updated `VoxelColliderState` snapshot. Settings are ordinary ECS components, and the
[`voxel_settings.lua`](../assets/scripts/voxel_settings.lua) script connects them to the panel. Scripts can read
and update the same fields through the standard ECS API:

```lua
local ids = assert(ecs.get_resource("scene.entities"))
local settings = assert(ecs.get(ids.active_level, "VoxelCollider"))
settings.enabled = true
settings.visible = true
settings.physics = true
ecs.set(ids.active_level, "VoxelCollider", settings)

local state = assert(ecs.get(ids.active_level, "VoxelColliderState"))
-- state.status: loading, ready, unavailable, or error
-- state.resolution: voxel size when ready; state.visible_voxels: current debug count
```

The legacy `VoxelCollider.player_radius` field remains accepted for older scenes; the runtime uses each entity's
`PhysicsBody.radius`. Lua movement intents carry the entity ID, and the host reconciles worker results into its
`PlayerCamera` or `Transform`. See [entity physics and Lua tuning](entity-physics.md) for per-entity controls,
collision events, gravity and script exports.

The generic ground-height query samples five downward rays through the voxel octree and returns
the median height when at least three samples find a surface. The player Lua script uses this
query for optional spawn alignment; Rust supplies geometry results without choosing a player pose.
An unavailable collider or an insufficient nearby floor leaves the original spawn unchanged.
