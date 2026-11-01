-- Physics is a normal fixed-tick Lua system. ECS components hold the settings;
-- the host resolves movement against the active level collider when available.
ecs.system("physics.integrate", function(dt)
    -- Read only the bindings needed this tick; the scene can retarget either
    -- entity without copying its entire ID map across the scripting bridge.
    local player = ecs.get_resource_field("scene.entities", "player")
    local level_entity = ecs.get_resource_field("scene.entities", "active_level")
    local body = player and ecs.get(player, "PhysicsBody")
    local level = level_entity and ecs.get(level_entity, "VoxelCollider")
    local state = level_entity and ecs.get(level_entity, "VoxelColliderState")
    local expects_ground = body and body.enabled and body.gravity_enabled and body.collision_enabled and
        level and level.enabled and level.physics and state
    local wait = ecs.get_resource("player.stand_probe_pending") == true or
        (expects_ground and (state.status ~= "ready" or ecs.get_resource("player.spawn_probe_pending") == true))
    -- A missing collider must not drop the player through an unloaded scan.
    -- Other entities and intentionally disabled collision keep normal gravity.
    engine.physics.step(dt, wait and player or nil)
end)

-- The worker reports resolved displacement; Lua defines how bodies respond.
ecs.on("physics.motion_resolved", function(event)
    if not ecs.is_alive(event.entity) then return end
    local body = ecs.get(event.entity, "PhysicsBody")
    if not body or not body.enabled or not body.collision_enabled then return end
    local collided = false
    for i = 1, 3 do
        local difference = math.abs(event.attempted[i] - event.moved[i])
        if difference > 0.0005 and event.attempted[i] * body.velocity[i] > 0 then body.velocity[i] = 0 end
        collided = collided or difference > 0.0005
    end
    if event.attempted[2] < -0.00001 then body.grounded = event.moved[2] - event.attempted[2] > 0.0005
    elseif event.attempted[2] > 0.00001 then body.grounded = false end
    ecs.set(event.entity, "PhysicsBody", body)
    if collided then ecs.emit("physics.collision", {entity=event.entity, attempted=event.attempted,
        moved=event.moved, grounded=body.grounded}) end
end)
ecs.on("physics.motion_error", function(event)
    if ecs.is_alive(event.entity) then
        local body = ecs.get(event.entity, "PhysicsBody")
        if body then
            for i = 1, 3 do body.velocity[i] = 0 end
            body.grounded = false; ecs.set(event.entity, "PhysicsBody", body)
        end
    end
    ecs.emit("physics.error", event)
end)
