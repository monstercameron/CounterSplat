-- Visual camera motion is a Lua behavior. The player position and capsule stay
-- untouched: only PlayerCamera.view_offset is sent to the renderer.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)

-- These exports are ordinary PlayerControls fields, so Entity settings edits
-- them directly and the existing controls persistence saves them per browser.
engine.export(player, "bob_enabled", {component="PlayerControls", field="bob_enabled", label="View bob"})
engine.export(player, "bob_cadence", {component="PlayerControls", field="bob_cadence", label="Bob steps / sec", min=0.2, max=6})
engine.export(player, "bob_vertical", {component="PlayerControls", field="bob_vertical", label="Bob vertical (m)", min=0, max=0.25})
engine.export(player, "bob_horizontal", {component="PlayerControls", field="bob_horizontal", label="Bob sideways (m)", min=0, max=0.25})
engine.export(player, "bob_smoothing", {component="PlayerControls", field="bob_smoothing", label="Bob response", min=1, max=30})

local phase, sideways, vertical = 0, 0, 0
local since_motion, resolved_distance = 1, 0
local previous = assert(ecs.get(player, "PlayerCamera")).position

-- Collision runs asynchronously. Only resolved horizontal travel counts, so
-- pressing against a wall doesn't keep the bob active.
ecs.on("physics.motion_resolved", function(event)
    if event.entity ~= player or type(event.moved) ~= "table" then return end
    local x, z = event.moved[1], event.moved[3]
    if type(x) ~= "number" or type(z) ~= "number" then return end
    local distance = math.sqrt(x * x + z * z)
    if distance == distance and distance < 10000000 then resolved_distance = resolved_distance + distance end
end)

local function reset()
    phase, sideways, vertical = 0, 0, 0
    since_motion, resolved_distance = 1, 0
    local camera = ecs.get(player, "PlayerCamera")
    if camera then
        previous = camera.position
        camera.view_offset = {0, 0, 0}
        ecs.set(player, "PlayerCamera", camera)
    end
end
ecs.on("input.reset", reset)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then reset() end
end)

ecs.system("player.camera_bob", function(dt)
    local camera = ecs.get(player, "PlayerCamera")
    local controls = ecs.get(player, "PlayerControls")
    local rig = ecs.get(player, "PlayerRig")
    if not camera or not controls or not rig then return end
    dt = math.max(0, math.min(dt, 0.25))
    if dt == 0 then return end

    local body = ecs.get(player, "PhysicsBody")
    local captured = ecs.get_resource("voxel.capture_movement") == true and body and body.enabled and body.collision_enabled
    local distance
    if captured then
        distance = resolved_distance
    else
        local dx, dz = camera.position[1] - previous[1], camera.position[3] - previous[3]
        distance = math.sqrt(dx * dx + dz * dz)
        -- Teleports and reset views are not footsteps.
        local limit = controls.move_speed * controls.sprint_multiplier * rig.units_per_meter * dt * 2 + 0.25
        if distance > limit then distance = 0 end
    end
    resolved_distance = 0
    previous = camera.position

    local grounded = not captured or not body.gravity_enabled or body.grounded
    local moving = controls.enabled and controls.bob_enabled and grounded
    since_motion = since_motion + dt
    if moving and distance > 0.00001 then
        since_motion = 0
        -- A full side-to-side cycle contains two steps; faster travel naturally
        -- increases cadence without tying it to the render or simulation rate.
        phase = (phase + math.pi * controls.bob_cadence * distance /
            (controls.move_speed * rig.units_per_meter)) % (2 * math.pi)
    end
    local active = moving and since_motion < 0.12
    local target_side = active and math.sin(phase) * controls.bob_horizontal * rig.units_per_meter or 0
    local target_up = active and math.sin(2 * phase) * controls.bob_vertical * rig.units_per_meter or 0
    local blend = 1 - math.exp(-controls.bob_smoothing * dt)
    sideways = sideways + (target_side - sideways) * blend
    vertical = vertical + (target_up - vertical) * blend
    if math.abs(sideways) < 0.000001 then sideways = 0 end
    if math.abs(vertical) < 0.000001 then vertical = 0 end

    -- Sideways follows the camera's horizontal right vector, independent of pitch.
    camera.view_offset = {math.cos(camera.yaw) * sideways, vertical, math.sin(camera.yaw) * sideways}
    ecs.set(player, "PlayerCamera", camera)
end)
