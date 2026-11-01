-- The player, UI, and settings are ordinary ECS components. The host supplies
-- ordered input events and runs this system at the engine's fixed simulation tick.
local ids = assert(ecs.get_resource("scene.entities"), "gameplay entities are missing")
local player = assert(ids.player, "player entity is missing")
local read_controls = engine.component_reader(player, "PlayerControls")
local read_rig = engine.component_reader(player, "PlayerRig")
local read_level = ids.active_level and engine.component_reader(ids.active_level, "VoxelCollider")
local read_level_state = ids.active_level and engine.component_reader(ids.active_level, "VoxelColliderState")

-- The host provides a validated level-camera candidate; the player script owns
-- the spawn policy and reset pose. Other hosts may omit the optional candidate
-- and provide a spawn directly, or let the authored PlayerCamera be the spawn.
local preview = ecs.get_resource("preview") or {}
local spawn_source = type(preview.player_spawn) == "string" and preview.player_spawn or "level"
assert(spawn_source == "level" or spawn_source == "scene", "preview.player_spawn must be level or scene")
local level_view = ecs.get_resource("player.level_view")
if spawn_source == "level" and type(level_view) == "table" then
    local candidate = level_view.camera
    assert(type(candidate) == "table" and candidate ~= ecs.null,
        "invalid level player camera: " .. tostring(level_view.error or "missing camera"))
    ecs.set(player, "PlayerCamera", candidate)
    ecs.set_resource("player.spawn_camera", candidate)
elseif spawn_source == "scene" or not ecs.get_resource("player.spawn_camera") then
    ecs.set_resource("player.spawn_camera", assert(ecs.get(player, "PlayerCamera")))
end

local held = {}
-- The free-fly player can make one ballistic hop at a time. The physics host
-- still resolves every movement intent against the active level's voxels.
local jump = nil
local waiting_for_key = nil
local sliders = { [ids.controls_speed] = "move_speed", [ids.controls_sprint] = "sprint_multiplier", [ids.controls_sensitivity] = "mouse_sensitivity", [ids.controls_fov] = "fov_degrees", [ids.controls_jump_speed] = "jump_speed", [ids.controls_jump_gravity] = "jump_gravity" }
local rig_sliders = { [ids.controls_eye_height] = "eye_height_m", [ids.controls_player_scale] = "units_per_meter" }
local checks = { [ids.controls_enabled] = "enabled", [ids.controls_mouse] = "mouse_look", [ids.controls_invert] = "invert_y" }
local keys = { [ids.key_forward] = "forward", [ids.key_backward] = "backward", [ids.key_left] = "left", [ids.key_right] = "right", [ids.key_up] = "up", [ids.key_down] = "down", [ids.key_sprint] = "sprint", [ids.key_jump] = "jump" }
if ids.key_crouch then keys[ids.key_crouch] = "crouch" end

local function notice(text)
    local element = ecs.get(ids.controls_notice, "UiElement")
    if element then element.widget.text = text; ecs.set(ids.controls_notice, "UiElement", element) end
end
local function key_label(code)
    if code:match("^Key%u$") then return code:sub(4) end
    if code:match("^Digit%d$") then return code:sub(6) end
    if code == "Space" then return "Space" end
    if code == "ShiftLeft" then return "Left Shift" end
    if code == "ShiftRight" then return "Right Shift" end
    if code == "ControlLeft" then return "Left Ctrl" end
    if code == "ControlRight" then return "Right Ctrl" end
    return code
end
local function sync_controls(controls)
    for id, field in pairs(sliders) do
        local element = ecs.get(id, "UiElement")
        local value = controls[field]
        if field == "fov_degrees" and value == ecs.null then value = ecs.get(player, "PlayerCamera").fov_degrees end
        element.widget.value = math.max(element.widget.min, math.min(element.widget.max, value))
        ecs.set(id, "UiElement", element)
    end
    local rig = assert(ecs.get(player, "PlayerRig"), "player rig is missing")
    for id, field in pairs(rig_sliders) do
        local element = ecs.get(id, "UiElement")
        element.widget.value = math.max(element.widget.min, math.min(element.widget.max, rig[field]))
        ecs.set(id, "UiElement", element)
    end
    for id, field in pairs(checks) do
        local element = ecs.get(id, "UiElement"); element.widget.checked = controls[field]; ecs.set(id, "UiElement", element)
    end
    for id, field in pairs(keys) do
        local element = ecs.get(id, "UiElement"); element.widget.label = key_label(controls.bindings[field]); ecs.set(id, "UiElement", element)
    end
end
-- Level calibration and saved preferences are data supplied by the host. Lua
-- chooses the final rig and keeps scene defaults available for reset.
local authored_rig = assert(ecs.get(player, "PlayerRig"), "player rig is missing")
local level_units = ecs.get_resource("player.level_units_per_meter")
if type(level_units) == "number" then authored_rig.units_per_meter = level_units end
ecs.set(player, "PlayerRig", authored_rig)
ecs.set_resource("player.default_rig", authored_rig)
local saved_rig = ecs.get_resource("player.saved_rig")
if type(saved_rig) == "table" then pcall(function() ecs.set(player, "PlayerRig", saved_rig) end) end

-- Exports are references to ECS fields, so the inspector edits the same values
-- the player system reads rather than a separate set of tuning variables.
engine.export(player, "move_speed", {component="PlayerControls", field="move_speed", label="Move speed", min=0.1, max=100})
engine.export(player, "eye_height", {component="PlayerRig", field="eye_height_m", label="Eye height (m)", min=0.7, max=2.5})
engine.export(player, "level_scale", {component="PlayerRig", field="units_per_meter", label="Scale vs level", min=0.5, max=2.5})
engine.export(player, "jump_speed", {component="PlayerControls", field="jump_speed", label="Jump speed", min=0.5, max=15})
engine.export(player, "invert_y", {component="PlayerControls", field="invert_y", label="Invert mouse Y"})
engine.export(player, "head_clearance", {component="PlayerRig", field="head_clearance_m", label="Head above eyes (m)", min=0, max=0.5})
local initial = assert(ecs.get(player, "PlayerControls"), "player settings are missing")
if initial.fov_degrees ~= ecs.null then
    local camera = ecs.get(player, "PlayerCamera"); camera.fov_degrees = initial.fov_degrees; ecs.set(player, "PlayerCamera", camera)
end
local initial_rig = assert(ecs.get(player, "PlayerRig"), "player rig is missing")
local last_rig = nil
-- Keep the camera-to-feet distance, collider and world-unit motion in agreement.
-- Changes preserve the current foot position; initial setup preserves the source view.
local function apply_rig(preserve_feet)
    local rig = assert(ecs.get(player, "PlayerRig"))
    if last_rig and last_rig.eye_height_m == rig.eye_height_m and
        last_rig.radius_m == rig.radius_m and last_rig.head_clearance_m == rig.head_clearance_m and
        last_rig.units_per_meter == rig.units_per_meter then return end
    local posture = ecs.get(player, "PlayerPosture")
    -- Defer size/scale edits while crouched; standing probes the new dimensions.
    if last_rig and posture and posture.crouched then return end
    local body = assert(ecs.get(player, "PhysicsBody"), "player physics is missing")
    local old_height = body.radius + (body.capsule_half_height or 0) - body.collision_offset_y
    local new_height = rig.eye_height_m * rig.units_per_meter
    local body_height = rig.eye_height_m + rig.head_clearance_m
    local previous_scale = last_rig and last_rig.units_per_meter or 1
    local ratio = rig.units_per_meter / previous_scale
    body.radius = rig.radius_m * rig.units_per_meter
    body.capsule_half_height = (body_height * 0.5 - rig.radius_m) * rig.units_per_meter
    body.collision_offset_y = (body_height * 0.5 - rig.eye_height_m) * rig.units_per_meter
    if ratio ~= 1 then
        body.gravity = math.max(0, math.min(100, body.gravity * ratio))
        for i = 1, 3 do body.velocity[i] = math.max(-200, math.min(200, body.velocity[i] * ratio)) end
    end
    ecs.set(player, "PhysicsBody", body)
    if preserve_feet then
        local camera = assert(ecs.get(player, "PlayerCamera"))
        local height_change = new_height - old_height
        camera.position[2] = math.max(-10000000, math.min(10000000, camera.position[2] + height_change))
        ecs.set(player, "PlayerCamera", camera)
        local spawn = ecs.get_resource("player.spawn_camera")
        if spawn then
            -- Spawn is standing even if a size edit was deferred during crouch.
            local spawn_change = last_rig and new_height - last_rig.eye_height_m * last_rig.units_per_meter or height_change
            spawn.position[2] = math.max(-10000000, math.min(10000000, spawn.position[2] + spawn_change))
            ecs.set_resource("player.spawn_camera", spawn)
        end
    end
    last_rig = {eye_height_m=rig.eye_height_m, head_clearance_m=rig.head_clearance_m,
        radius_m=rig.radius_m, units_per_meter=rig.units_per_meter}
end
apply_rig(false)
sync_controls(initial)
ecs.system("player.rig", function() apply_rig(true) end)

-- Imported viewer cameras are not necessarily player-height. Probe only a
-- nearby floor and reject late results after movement or an authored scene pose.
local pending_ground = nil
local ground_sequence = 0
ecs.set_resource("player.spawn_probe_pending", false)

-- Hold a gravity-driven player while the level collider and initial ground
-- probe are loading. On levels without collision, allow free-fly navigation.
local function level_collision(body)
    if not body or not body.enabled or not body.gravity_enabled or not body.collision_enabled then return nil end
    local level = read_level and read_level()
    local state = read_level_state and read_level_state()
    if not level or not level.enabled or not level.physics or not state then return nil end
    return state.status
end
local function waiting_for_ground(body)
    local status = level_collision(body)
    return status == "loading" or (status == "ready" and ecs.get_resource("player.spawn_probe_pending") == true)
end
local function gravity_active(body)
    local status = level_collision(body)
    return body and body.enabled and body.gravity_enabled and
        status ~= "loading" and status ~= "unavailable" and status ~= "error"
end
ecs.system("player.ground_probe_timeout", function(dt)
    if pending_ground then
        pending_ground.elapsed = pending_ground.elapsed + math.max(0, dt)
        if pending_ground.elapsed >= 5 then
            pending_ground = nil
            ecs.set_resource("player.spawn_probe_pending", false)
        end
    end
end)
local function near(a, b)
    for i = 1, 3 do if math.abs(a[i] - b[i]) >= 0.001 then return false end end
    return true
end
ecs.on("physics.ready", function()
    local rig = ecs.get(player, "PlayerRig")
    local camera = ecs.get(player, "PlayerCamera")
    local body = ecs.get(player, "PhysicsBody")
    local spawn = ecs.get_resource("player.spawn_camera")
    local preview = ecs.get_resource("preview") or {}
    if not rig or not rig.auto_align_spawn or not camera or not body or not spawn or
        preview.player_spawn == "scene" or not near(camera.position, spawn.position) then return end
    ground_sequence = ground_sequence + 1
    local token = "player-spawn-" .. ground_sequence
    pending_ground = {token=token, position=camera.position, version=engine.physics.motion_version(player), elapsed=0}
    ecs.set_resource("player.spawn_probe_pending", true)
    engine.physics.probe_ground(player, camera.position, math.min(1, body.radius * 0.5),
        math.min(8, 4 * rig.units_per_meter), token)
end)
ecs.on("physics.ground_result", function(result)
    if not pending_ground or result.entity ~= player or result.token ~= pending_ground.token then return end
    local request = pending_ground; pending_ground = nil
    ecs.set_resource("player.spawn_probe_pending", false)
    local rig = ecs.get(player, "PlayerRig")
    local camera = ecs.get(player, "PlayerCamera")
    local spawn = ecs.get_resource("player.spawn_camera")
    if not rig or not rig.auto_align_spawn or not camera or not spawn or type(result.height) ~= "number" or
        request.version ~= engine.physics.motion_version(player) or not near(camera.position, request.position) or
        not near(spawn.position, request.position) then return end
    local eye = result.height + rig.eye_height_m * rig.units_per_meter
    if eye ~= eye or math.abs(eye) > 10000000 then return end
    camera.position[2] = eye; spawn.position[2] = eye
    ecs.set(player, "PlayerCamera", camera)
    ecs.set_resource("player.spawn_camera", spawn)
end)

local controls_refresh = 0
ecs.system("player.controls_ui_sync", function(dt)
    controls_refresh = controls_refresh + dt
    if controls_refresh < 0.25 then return end
    controls_refresh = 0
    local panel = ecs.get(ids.controls_panel, "UiElement")
    if panel and not panel.collapsible.collapsed and not waiting_for_key then
        sync_controls(ecs.get(player, "PlayerControls"))
    end
end)

local function cancel_binding()
    waiting_for_key = nil
    ecs.set_resource("input.binding_capture", false)
    sync_controls(ecs.get(player, "PlayerControls"))
end
ecs.on("input.reset", function()
    held = {}
    if waiting_for_key then cancel_binding(); notice("") end
end)
ecs.on("input.key", function(event)
    if waiting_for_key then
        if not event.pressed or event.is_repeat then return end
        if event.key == "Escape" then cancel_binding(); notice("Binding cancelled"); return end
        local code = event.code
        if type(code) ~= "string" or #code == 0 or #code > 32 or not code:match("^[%a%d]+$") then
            notice("That key cannot be used as a binding")
            return
        end
        local controls = ecs.get(player, "PlayerControls")
        controls.bindings[waiting_for_key] = code
        local ok = pcall(function() ecs.set(player, "PlayerControls", controls) end)
        if ok then
            cancel_binding(); notice(""); held = {}; ecs.emit("player.controls_changed", controls)
        else notice("That key cannot be used as a binding") end
        return
    end
    if type(event.code) ~= "string" then return end
    if not event.pressed then
        held[event.code] = nil
    elseif event.gameplay and not event.is_repeat then
        if not held[event.code] then
            local settings = ecs.get(player, "PlayerControls")
            if settings and settings.enabled and event.code == settings.bindings.jump and not jump then
                local body = ecs.get(player, "PhysicsBody")
                if waiting_for_ground(body) then return end
                if gravity_active(body) then
                    if body.grounded then
                        local rig = assert(ecs.get(player, "PlayerRig"))
                        body.velocity[2] = settings.jump_speed * rig.units_per_meter
                        body.grounded = false
                        ecs.set(player, "PhysicsBody", body)
                    end
                else
                    local rig = assert(ecs.get(player, "PlayerRig"))
                    jump = { elapsed = 0, offset = 0, speed = settings.jump_speed * rig.units_per_meter,
                        gravity = settings.jump_gravity * rig.units_per_meter }
                end
            end
        end
        held[event.code] = true
    end
end)
ecs.on("input.mouse_delta", function(event)
    -- A scene can install its own camera controller. Older scenes keep this
    -- direct-look fallback, and disabling the controller restores it.
    local owner = ecs.get_resource("camera.look_owner")
    local controller = owner and ecs.is_alive(owner) and ecs.get(owner, "CameraController")
    local target = controller and ids[controller.target]
    if controller and controller.enabled and target and ecs.has(target, "PlayerCamera") then return end
    local settings = ecs.get(player, "PlayerControls")
    if not settings or not settings.enabled or not settings.mouse_look then return end
    local camera = ecs.get(player, "PlayerCamera")
    local dx = math.max(-2000, math.min(2000, event.dx or 0))
    local dy = math.max(-2000, math.min(2000, event.dy or 0))
    camera.yaw = (camera.yaw + dx * settings.mouse_sensitivity + math.pi) % (2 * math.pi) - math.pi
    local vertical = settings.invert_y and 1 or -1
    camera.pitch = math.max(-1.553343, math.min(1.553343, camera.pitch + dy * settings.mouse_sensitivity * vertical))
    ecs.set_fields(player, "PlayerCamera", {yaw=camera.yaw, pitch=camera.pitch})
end)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then
        local spawn = ecs.get_resource("player.spawn_camera")
        local settings = ecs.get(player, "PlayerControls")
        if spawn then
            if settings.fov_degrees ~= ecs.null then spawn.fov_degrees = settings.fov_degrees end
            ecs.set(player, "PlayerCamera", spawn)
        end
        held = {}; jump = nil
        local body = ecs.get(player, "PhysicsBody")
        if body then body.velocity = ecs.array(); body.velocity[1] = 0; body.velocity[2] = 0; body.velocity[3] = 0; body.grounded = false; ecs.set(player, "PhysicsBody", body) end
        return
    end
    local controls = ecs.get(player, "PlayerControls")
    if keys[action.id] and action.kind.type == "clicked" then
        waiting_for_key = keys[action.id]
        ecs.set_resource("input.binding_capture", true)
        sync_controls(controls)
        local element = ecs.get(action.id, "UiElement"); element.widget.label = "Press a key"; ecs.set(action.id, "UiElement", element)
        notice("Press a key, or Escape to cancel"); held = {}
        return
    end
    if action.id == ids.controls_reset and action.kind.type == "clicked" then
        waiting_for_key = nil; ecs.set_resource("input.binding_capture", false)
        controls = assert(ecs.get_resource("player.default_controls"))
        ecs.set(player, "PlayerControls", controls)
        local default_rig = ecs.get_resource("player.default_rig") or initial_rig
        ecs.set(player, "PlayerRig", default_rig)
        apply_rig(true)
        ecs.emit("player.rig_changed", default_rig)
        local camera = ecs.get(player, "PlayerCamera")
        camera.fov_degrees = ecs.get_resource("player.spawn_camera").fov_degrees
        ecs.set(player, "PlayerCamera", camera)
        sync_controls(controls); notice("Defaults restored"); held = {}; jump = nil
        ecs.emit("player.controls_changed", controls)
        return
    end
    local rig_field = rig_sliders[action.id]
    if rig_field and action.kind.type == "number_changed" then
        local rig = assert(ecs.get(player, "PlayerRig"))
        rig[rig_field] = action.kind.value
        local ok = pcall(function() ecs.set(player, "PlayerRig", rig) end)
        if ok then
            apply_rig(true); sync_controls(controls); notice(""); held = {}
            ecs.emit("player.rig_changed", rig)
        else notice("Invalid player scale") end
        return
    end
    local field = sliders[action.id]
    if field and action.kind.type == "number_changed" then controls[field] = action.kind.value
    else
        field = checks[action.id]
        if field and action.kind.type == "bool_changed" then controls[field] = action.kind.value
        else
            return
        end
    end
    local ok = pcall(function() ecs.set(player, "PlayerControls", controls) end)
    if ok then
        if field == "fov_degrees" then
            local camera = ecs.get(player, "PlayerCamera"); camera.fov_degrees = controls.fov_degrees; ecs.set(player, "PlayerCamera", camera)
        end
        notice(""); held = {}; ecs.emit("player.controls_changed", controls)
    else notice("Invalid control value") end
end)

ecs.system("player.camera", function(dt)
    local settings = read_controls()
    if not settings or not settings.enabled then jump = nil; return end
    if ecs.get_resource("player.stand_probe_pending") == true then return end
    local b = settings.bindings
    local function down(code) return engine.input.is_down(held, code) and 1 or 0 end
    local forward = down(b.forward) - down(b.backward)
    local right = down(b.right) - down(b.left)
    local up = down(b.up) - down(b.down)
    local length = math.sqrt(forward * forward + right * right + up * up)
    local hop = 0
    local body = ecs.get(player, "PhysicsBody")
    if waiting_for_ground(body) then jump = nil; return end
    if gravity_active(body) then jump = nil end
    if jump then
        -- Integrate the arc analytically so its height and duration do not depend on the tick rate.
        local duration = 2 * jump.speed / jump.gravity
        jump.elapsed = math.min(duration, jump.elapsed + math.max(0, dt))
        local offset = math.max(0, jump.speed * jump.elapsed - 0.5 * jump.gravity * jump.elapsed * jump.elapsed)
        hop = offset - jump.offset
        jump.offset = offset
        if jump.elapsed >= duration then jump = nil end
    end
    if length == 0 and hop == 0 then return end
    local camera = ecs.get(player, "PlayerCamera")
    local rig = assert(read_rig())
    local posture = ecs.get(player, "PlayerPosture")
    local crouched = posture and (posture.crouched or (posture.amount or 0) > 0.001)
    local multiplier = engine.input.is_down(held, b.sprint) and settings.sprint_multiplier or 1
    if crouched then
        local factor = posture.crouch_speed_multiplier
        multiplier = type(factor) == "number" and factor == factor and math.max(0.1, math.min(1, factor)) or 0.45
    end
    local scale = rig.units_per_meter
    if posture and posture.crouched and type(posture.active_units_per_meter) == "number" then
        scale = posture.active_units_per_meter
    end
    local speed = length > 0 and settings.move_speed * scale * multiplier * dt / length or 0
    local sine, cosine = math.sin(camera.yaw), math.cos(camera.yaw)
    local delta = ecs.array()
    delta[1] = (forward * sine + right * cosine) * speed
    delta[2] = up * speed + hop
    delta[3] = (-forward * cosine + right * sine) * speed
    engine.physics.move(player, delta)
end)
