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

local phase, sideways, vertical, pitch, roll, run_blend = 0, 0, 0, 0, 0, 0
local since_motion, resolved_distance = 1, 0
local held, aiming = {}, false

-- The same asymmetric foot-plant curve used by Neural-Sight's running view.
local function running_step_height(phase_value)
    local p = (phase_value / math.pi) % 1
    if p < 0.62 then return -math.cos(math.pi * p / 0.62) end
    return math.cos(math.pi * (p - 0.62) / 0.38)
end
ecs.on("input.key", function(event)
    if type(event.code) ~= "string" then return end
    if not event.pressed then held[event.code] = nil
    elseif event.gameplay and not event.is_repeat then held[event.code] = true end
end)
ecs.on("input.mouse_button", function(event)
    if event.button == "right" and (event.gameplay or not event.pressed) then aiming = event.pressed == true end
end)
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
    phase, sideways, vertical, pitch, roll, run_blend = 0, 0, 0, 0, 0, 0
    since_motion, resolved_distance = 1, 0
    held, aiming = {}, false
    local camera = ecs.get(player, "PlayerCamera")
    if camera then
        previous = camera.position
        camera.view_offset = {0, 0, 0}
        camera.view_bob_pitch, camera.view_bob_roll = 0, 0
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
    local profile = ids.camera_controller and ecs.is_alive(ids.camera_controller) and
        ecs.get(ids.camera_controller, "CameraController")
    if profile and (not profile.enabled or ids[profile.target] ~= player) then profile = nil end
    local posture = ecs.get(player, "PlayerPosture")
    local crouched = posture and (posture.crouched or (posture.amount or 0) > 0.001)
    local running = profile and not crouched and engine.input.is_down(held, controls.bindings.sprint)
    since_motion = since_motion + dt
    if moving and distance > 0.00001 then
        since_motion = 0
        -- Two footfalls per sway cycle; the longer run stride follows Neural-Sight.
        local stride_scale = running and (0.85 / 1.4) or 1
        phase = (phase + math.pi * controls.bob_cadence * distance * stride_scale /
            (controls.move_speed * rig.units_per_meter)) % (2 * math.pi)
    end
    local active = moving and since_motion < 0.12
    local blend = 1 - math.exp(-controls.bob_smoothing * dt)
    run_blend = run_blend + ((running and 1 or 0) - run_blend) * blend
    local gain = active and (crouched and 0.45 or 1) or 0
    if profile then
        -- Only actual resolved travel drives the camera; aiming damps both
        -- positional and angular gait while preserving direct mouse aim.
        local nominal = controls.move_speed * (running and controls.sprint_multiplier or 1) * rig.units_per_meter
        gain = gain * math.min(1, distance / math.max(0.000001, dt * nominal))
        if aiming then gain = gain * profile.aim_bob_scale end
    end
    local wave = math.sin(2 * phase)
    if profile then wave = -math.cos(2 * phase) * (1 - run_blend) + running_step_height(phase) * run_blend end
    local side_wave = math.sin(phase)
    local target_side = side_wave * controls.bob_horizontal * rig.units_per_meter * gain * (1 + 1.166667 * run_blend)
    local target_up = wave * controls.bob_vertical * rig.units_per_meter * gain * (1 + 1.5 * run_blend)
    local angular = profile and profile.gait_rotation_strength or 0
    local target_pitch = -wave * math.rad(0.10 + 0.22 * run_blend) * gain * angular
    local target_roll = side_wave * math.rad(0.075 + 0.205 * run_blend) * gain * angular
    sideways = sideways + (target_side - sideways) * blend
    vertical = vertical + (target_up - vertical) * blend
    pitch = pitch + (target_pitch - pitch) * blend
    roll = roll + (target_roll - roll) * blend
    if math.abs(sideways) < 0.000001 then sideways = 0 end
    if math.abs(vertical) < 0.000001 then vertical = 0 end
    if math.abs(pitch) < 0.000001 then pitch = 0 end
    if math.abs(roll) < 0.000001 then roll = 0 end

    -- These offsets never feed back into the player position or gameplay aim.
    local x, z = math.cos(camera.yaw) * sideways, math.sin(camera.yaw) * sideways
    local offset = camera.view_offset
    if offset[1] ~= x or offset[2] ~= vertical or offset[3] ~= z or
        camera.view_bob_pitch ~= pitch or camera.view_bob_roll ~= roll then
        camera.view_offset = {x, vertical, z}
        camera.view_bob_pitch, camera.view_bob_roll = pitch, roll
        ecs.set(player, "PlayerCamera", camera)
    end
end)
