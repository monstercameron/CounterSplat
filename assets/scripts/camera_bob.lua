-- Visual camera motion is a Lua behavior. The player position and capsule stay
-- untouched: only PlayerCamera.view_offset is sent to the renderer.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)
local read_controls = engine.component_reader(player, "PlayerControls")
local read_rig = engine.component_reader(player, "PlayerRig")
local read_posture = engine.component_reader(player, "PlayerPosture")
local read_sprint = engine.component_reader(player, "SprintVisuals")
local read_weapon_state = ids.first_person and engine.component_reader(ids.first_person, "WeaponState")
local read_controller = ids.camera_controller and engine.component_reader(ids.camera_controller, "CameraController")
local read_gait = ecs.get(player, "PlayerGait") and engine.component_reader(player, "PlayerGait")

-- These exports are ordinary PlayerControls fields, so Entity settings edits
-- them directly and the existing controls persistence saves them per browser.
engine.export(player, "bob_enabled", {component="PlayerControls", field="bob_enabled", label="View bob"})
engine.export(player, "bob_cadence", {component="PlayerControls", field="bob_cadence", label="Walk steps / sec", min=0.2, max=6})
engine.export(player, "bob_vertical", {component="PlayerControls", field="bob_vertical", label="Bob vertical (m)", min=0, max=0.25})
engine.export(player, "bob_horizontal", {component="PlayerControls", field="bob_horizontal", label="Bob sideways (m)", min=0, max=0.25})
engine.export(player, "bob_smoothing", {component="PlayerControls", field="bob_smoothing", label="Bob response", min=1, max=30})
engine.export(player, "breathing_enabled", {component="PlayerControls", field="breathing_enabled", label="Breathing sway"})

local function bounded(value, fallback, low, high)
    if type(value)~="number" or value~=value or math.abs(value)>1000000 then return fallback end
    return math.max(low, math.min(high,value))
end

-- Sprint presentation is optional scene data; the renderer consumes only the
-- transient blur amount, leaving movement and tuning in the Lua/ECS layer.
local sprint_config = ecs.get(player, "SprintVisuals")
if sprint_config then
    local defaults = {peripheral_enabled=true, peripheral_strength=0.45, peripheral_response=8,
        vertical_bob_multiplier=1.4, horizontal_bob_multiplier=1.35}
    local entries = {
        {"sprint_blur_enabled","peripheral_enabled","Sprint peripheral blur"},
        {"sprint_blur_strength","peripheral_strength","Sprint blur strength",0,1},
        {"sprint_blur_response","peripheral_response","Sprint blur response",1,30},
        {"sprint_bob_vertical","vertical_bob_multiplier","Run bob vertical",0,3},
        {"sprint_bob_horizontal","horizontal_bob_multiplier","Run bob sideways",0,3},
    }
    local changed = false
    for _, spec in ipairs(entries) do
        local field, value = spec[2], sprint_config[spec[2]]
        if spec[4] then value = bounded(value,defaults[field],spec[4],spec[5])
        elseif type(value) ~= "boolean" then value = defaults[field] end
        if sprint_config[field] ~= value then sprint_config[field] = value; changed = true end
    end
    if changed then ecs.set(player, "SprintVisuals", sprint_config) end
    for _, spec in ipairs(entries) do
        local entry={component="SprintVisuals",field=spec[2],label=spec[3]}
        if spec[4] then entry.min,entry.max=spec[4],spec[5] end
        engine.export(player,spec[1],entry)
    end
end

local phase, sideways, vertical, pitch, roll, run_blend = 0, 0, 0, 0, 0, 0
local since_motion, since_ground, resolved_distance, speed_ratio, peripheral_blur = 1, 1, 0, 0, 0
-- Steps: `steps` counts footfalls continuously; each whole step emits `player.step`. Breathing: `exertion`
-- (0..1) climbs while running, drifts to a light level while walking and settles at rest.
local steps, next_step, exertion, breath_phase, published_exertion = 0, 1, 0, 0, nil
local GAIT_CADENCE = {walk=2.0, run=2.8, cautious=1.3, crouch=1.5, slide=0}
local GAIT_BOB = {walk=1, run=1, cautious=0.5, crouch=0.45, slide=0.3}
local published_blur = nil
local function publish_blur(value)
    if published_blur == nil or math.abs(value-published_blur) >= 0.001 or (value==0 and published_blur~=0) then
        published_blur=value
        ecs.set_resource("camera.peripheral_blur",value)
    end
end
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
ecs.on("weapon.aim", function(event) aiming = event.pressed == true end)
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
    since_motion, since_ground, resolved_distance, speed_ratio, peripheral_blur = 1, 1, 0, 0, 0
    steps, next_step, exertion, breath_phase = 0, 1, 0, 0
    held, aiming = {}, false
    publish_blur(0)
    local camera = ecs.get(player, "PlayerCamera")
    if camera then
        previous = camera.position
        camera.view_offset = {0, 0, 0}
        camera.view_bob_pitch, camera.view_bob_roll = 0, 0
        ecs.set_fields(player, "PlayerCamera", {view_offset=camera.view_offset,
            view_bob_pitch=camera.view_bob_pitch, view_bob_roll=camera.view_bob_roll})
    end
end
ecs.on("input.reset", reset)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then reset() end
end)

ecs.system("player.camera_bob", function(dt)
    local camera, controls, rig = ecs.get(player, "PlayerCamera"), read_controls(), read_rig()
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

    -- Voxel floors can briefly report ungrounded while stepping over an
    -- uneven cell. Ignore those short gaps, but stop immediately on a jump.
    local on_ground = not captured or not body.gravity_enabled or body.grounded
    since_ground = on_ground and 0 or since_ground + dt
    local rising = captured and type(body.velocity) == "table" and
        type(body.velocity[2]) == "number" and body.velocity[2] > 0.25 * rig.units_per_meter
    local grounded = on_ground or (not rising and since_ground < 0.16)
    local traveling = controls.enabled and grounded
    local profile = read_controller and ecs.is_alive(ids.camera_controller) and read_controller()
    if profile and (not profile.enabled or ids[profile.target] ~= player) then profile = nil end
    local posture = read_posture()
    local crouched = posture and (posture.crouched or (posture.amount or 0) > 0.001)
    local active_aim = aiming
    if active_aim and read_weapon_state then
        local state = read_weapon_state()
        local name = type(state) == "table" and state.state or nil
        if name == "noGun" or name == "stow" or name == "equip" or name == "death" or
           name == "dead" or name == "reload" or name == "repack" then active_aim = false end
    end
    local sprinting = traveling and not crouched and not active_aim and engine.input.is_down(held, controls.bindings.sprint)
    since_motion = since_motion + dt
    if traveling and distance > 0.00001 then since_motion = 0 end
    local active = traveling and since_motion < 0.20
    local running = sprinting and active
    -- Collision responses may deliver several ticks of travel at once. Smooth
    -- speed rather than driving bob from a single response or an empty tick.
    local nominal_walk = math.max(0.000001, controls.move_speed * rig.units_per_meter)
    -- Do not discard large worker batches: they contain real travel accrued
    -- over several ticks. A longer response averages the arrival jitter.
    local sample = traveling and bounded(distance / (dt * nominal_walk), 0, 0, 60) or 0
    speed_ratio = speed_ratio + (sample - speed_ratio) * (1 - math.exp(-dt * 4))
    -- Simulated footsteps: each gait has a cadence (steps per second at its forward speed, scaled so walking
    -- matches `bob_cadence`), which sets its stride. Travel advances the step count by distance over stride,
    -- so steps slow when strafing or backing up; the bob's phase is half a cycle per step.
    local gait_state = ecs.get_resource("player.gait")
    local gait = type(gait_state) == "table" and type(gait_state.gait) == "string" and gait_state.gait or
        (crouched and "crouch" or running and "run" or "walk")
    -- The movement script's published gait is authoritative (a cautious walk overrides Shift, for one).
    if type(gait_state) == "table" and type(gait_state.gait) == "string" then running = active and gait == "run" end
    local gait_cfg = read_gait and read_gait() or nil
    local cadences = gait_cfg and type(gait_cfg.cadence_hz) == "table" and gait_cfg.cadence_hz or {}
    local walk_cadence = bounded(cadences.walk, GAIT_CADENCE.walk, 0.1, 10)
    local cadence = bounded(cadences[gait], GAIT_CADENCE[gait] or walk_cadence, 0, 10) *
        bounded(controls.bob_cadence, 2, 0.2, 6) / walk_cadence
    local gait_speed = controls.move_speed * (gait == "run" and controls.sprint_multiplier or
        gait == "cautious" and bounded(gait_cfg and gait_cfg.cautious_multiplier, 0.5, 0.1, 1) or
        gait == "crouch" and bounded(posture and posture.crouch_speed_multiplier, 0.45, 0.1, 1) or 1)
    if active and distance > 0.00001 and cadence > 0 then
        local advance = distance / math.max(0.000001, gait_speed * rig.units_per_meter / cadence)
        steps = steps + advance
        if controls.bob_enabled then phase = (phase + math.pi * advance) % (2 * math.pi) end
        while steps >= next_step do
            ecs.emit("player.step", {player=player, foot=next_step % 2 == 1 and "left" or "right", gait=gait,
                speed_mps=type(gait_state) == "table" and gait_state.speed_mps or nil})
            next_step = next_step + 1
        end
    end
    local blend = 1 - math.exp(-controls.bob_smoothing * dt)
    run_blend = run_blend + ((running and 1 or 0) - run_blend) * blend
    -- The bob grows with ground speed (relative to walking) and is scaled per gait: a cautious walk barely moves
    -- the view, a run swings it hardest.
    local bob_scales = gait_cfg and type(gait_cfg.bob_scale) == "table" and gait_cfg.bob_scale or {}
    local gait_bob = bounded(bob_scales[gait], GAIT_BOB[gait] or 1, 0, 3)
    local speed_gain = bounded(math.sqrt(math.max(0, speed_ratio)), 1, 0.5, 1.6)
    local gain = controls.bob_enabled and active and gait_bob * speed_gain or 0
    if profile then
        gain = gain * math.min(1, speed_ratio / (running and controls.sprint_multiplier or 1))
        if active_aim then gain = gain * profile.aim_bob_scale end
    end

    local sprint = read_sprint()
    local strength = sprint and sprint.peripheral_enabled ~= false and
        bounded(sprint.peripheral_strength,0.45,0,1) or 0
    local response = sprint and bounded(sprint.peripheral_response,8,1,30) or 8
    local speed_range = math.max(0.05, controls.sprint_multiplier - 1)
    local intensity = running and math.max(0, math.min(1,(speed_ratio-1)/speed_range)) or 0
    local blur_target = strength * intensity
    peripheral_blur = peripheral_blur + (blur_target-peripheral_blur) * (1-math.exp(-response*dt))
    if peripheral_blur < 0.0005 and blur_target == 0 then peripheral_blur = 0 end
    publish_blur(peripheral_blur)

    local wave = math.sin(2 * phase)
    if profile then wave = -math.cos(2 * phase) * (1 - run_blend) + running_step_height(phase) * run_blend end
    local side_wave = math.sin(phase)
    local horizontal_run = sprint and bounded(sprint.horizontal_bob_multiplier,1.35,0,3) or 1.35
    local vertical_run = sprint and bounded(sprint.vertical_bob_multiplier,1.4,0,3) or 1.4
    local target_side = side_wave * controls.bob_horizontal * rig.units_per_meter * gain * (1 + (horizontal_run-1) * run_blend)
    local target_up = wave * controls.bob_vertical * rig.units_per_meter * gain * (1 + (vertical_run-1) * run_blend)
    local angular = profile and profile.gait_rotation_strength or 0
    local target_pitch = -wave * math.rad(0.10 + 0.22 * run_blend) * gain * angular
    local target_roll = side_wave * math.rad(0.075 + 0.205 * run_blend) * gain * angular

    -- Breathing: exertion climbs while running, drifts to a light level while walking and settles at rest; the
    -- breath gets faster and deeper with it, and aiming steadies it. It has its own toggle (breathing_enabled).
    local breath = gait_cfg and type(gait_cfg.breathing) == "table" and gait_cfg.breathing or {}
    local build = bounded(breath.run_build_seconds, 8, 0.5, 60)
    local recovery = bounded(breath.recovery_seconds, 6, 0.5, 60)
    if running then
        exertion = math.min(1, exertion + dt / build)
    elseif active then
        local level = bounded(breath.walk_level, 0.25, 0, 1)
        if exertion > level then exertion = math.max(level, exertion * math.exp(-dt / (2 * recovery)))
        else exertion = math.min(level, exertion + dt / (3 * build)) end
    else
        exertion = exertion * math.exp(-dt / recovery)
    end
    if exertion < 0.0001 then exertion = 0 end
    local rounded = math.floor(exertion * 1000 + 0.5) / 1000
    if rounded ~= published_exertion then published_exertion = rounded; ecs.set_resource("player.exertion", rounded) end
    local rest_rate = bounded(breath.rest_rate_hz, 0.22, 0.05, 2)
    breath_phase = (breath_phase + 2 * math.pi * (rest_rate + (bounded(breath.max_rate_hz, 0.75, 0.05, 3) - rest_rate) *
        exertion) * dt) % (2 * math.pi)
    if controls.breathing_enabled ~= false then
        local steady = active_aim and bounded(breath.aim_scale, 0.35, 0, 1) or 1
        local rest_amp, rest_tilt = bounded(breath.rest_amplitude_m, 0.0015, 0, 0.05), bounded(breath.rest_pitch_deg, 0.08, 0, 3)
        local amplitude = (rest_amp + (bounded(breath.max_amplitude_m, 0.009, 0, 0.05) - rest_amp) * exertion) * steady
        local tilt = (rest_tilt + (bounded(breath.max_pitch_deg, 0.55, 0, 3) - rest_tilt) * exertion) * steady
        local inhale = math.sin(breath_phase)
        target_up = target_up + inhale * amplitude * rig.units_per_meter
        target_pitch = target_pitch + inhale * math.rad(tilt)
    end
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
        ecs.set_fields(player, "PlayerCamera", {view_offset=camera.view_offset,
            view_bob_pitch=camera.view_bob_pitch, view_bob_roll=camera.view_bob_roll})
    end
end)
