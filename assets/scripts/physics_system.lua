-- Physics is a normal fixed-tick Lua system. ECS components hold the settings;
-- the host resolves movement against the active level collider when available.
ecs.system("physics.integrate", function(dt)
    engine.physics.step(dt)
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
