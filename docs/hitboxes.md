# Lua hitboxes

`assets/scripts/hitboxes.lua` exposes `engine.hitbox.trace_segment(entity, hitbox, start, finish)`. It returns the closest hit as `{target, fraction, position, normal, zone}` or `nil`. Ballistics compares that hit with the level voxel trace, ignores the shooter, and includes the zone on `projectile.impact` and `combat.damage`. Zone names are metadata: damage remains unchanged until gameplay scripts apply their own rules.

The gameplay player has a `Hitbox` component with nine named boxes: head, neck, chest, abdomen, pelvis, left and right arms, and left and right legs. Centers and half-extents are in metres relative to the feet, with X to the side, Y up, and Z forward. A `Transform` entity uses its position as that anchor; a player camera derives the feet from its `PhysicsBody`. Optional `yaw` rotates boxes around the vertical axis; otherwise a `PlayerCamera` supplies yaw. `scale_from_player_rig` applies the rig's world units per metre, and `follow_crouch` adjusts height using the actual collision capsule. Zones are gameplay hit detection, not physical colliders or a visible character model. Existing spherical `{radius, offset}` hitboxes continue to work.

Other entities can reuse the same component and API. Configure their zone data in scene JSON or Lua and register the hitbox script before ballistics. Every segment chooses the nearest intersected zone, including starts inside a box; identical distances use declaration order. Validation ignores malformed boxes, and at most 64 zones are considered per entity.


## Dojo shooting range

The Barn contains three scene-owned practice targets at different distances. Each target copies the player's nine body zones into its own ECS Hitbox and uses the same Lua transform for collision and visible world-space boxes. The range is scoped by training_range.level_ids in the gameplay scene, so other levels do not spawn active targets. A hit reports its body zone, hip/ADS stance, distance, and projectile travel time; the struck zone briefly brightens. Shots still respect level voxel collision and projectile travel.

Press **T** in gameplay to move the range in front of the player and restore the targets' health. The ECS inspector exposes each TrainingTarget's enabled state, hitbox visibility, distance, and sideways offset. Hiding a target's boxes does not disable its hit detection; disabling the target does. Range configuration and behavior live in gameplay.json and training_target.lua.

engine.hitbox.frame resolves the common feet anchor, rotation, scale, and crouch height. engine.hitbox.visual_segments derives bounded box edges from that same frame for any script or host that needs a debug visualization. Hitbox.units_per_meter, when present, sets an explicit scale; otherwise the optional player-rig scale remains available. The WebGPU host renders line snapshots without deciding gameplay hits.
