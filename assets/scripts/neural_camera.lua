-- First-person look and turn tilt adapted from Neural-Sight's main.js and
-- turn-tilt.js. The controller is a scene entity, and all behavior stays in Lua.
local ids = assert(ecs.get_resource("scene.entities"))
local owner = assert(ids.camera_controller, "camera controller entity is missing")
local read_controller = engine.component_reader(owner, "CameraController")
local read_player_controls = ids.player and engine.component_reader(ids.player, "PlayerControls")
local initial = assert(ecs.get(owner, "CameraController"))
ecs.set_resource("camera.look_owner", owner)

local exports = {
    {"camera_enabled", "enabled", "Neural camera"},
    {"camera_pitch_limit", "pitch_limit_degrees", "Pitch limit (deg)", 1, 89},
    {"camera_turn_enabled", "turn_tilt_enabled", "Turn tilt"},
    {"camera_turn_strength", "turn_tilt_strength", "Turn tilt strength", 0, 3},
    {"camera_turn_max", "turn_tilt_max_degrees", "Max turn tilt (deg)", 0, 5},
    {"camera_turn_rate", "turn_rate_response", "Turn rate response", 1, 40},
    {"camera_turn_response", "turn_tilt_response", "Turn tilt response", 1, 40},
    {"camera_aim_tilt", "aim_tilt_scale", "Aim tilt scale", 0, 1},
    {"camera_gait_rotation", "gait_rotation_strength", "Gait rotation", 0, 3},
    {"camera_aim_bob", "aim_bob_scale", "Aim bob scale", 0, 1},
}
for _, item in ipairs(exports) do
    local spec = {component="CameraController", field=item[2], label=item[3]}
    if item[4] then spec.min, spec.max = item[4], item[5] end
    engine.export(owner, item[1], spec)
end

local delta, rate, roll, aiming, previous = 0, 0, 0, false, nil
local function clear_target(entity)
    local camera = entity and ecs.is_alive(entity) and ecs.get(entity, "PlayerCamera")
    if camera and camera.view_turn_roll ~= 0 then
        ecs.set_field(entity, "PlayerCamera", "view_turn_roll", 0)
    end
end
local function reset_motion()
    delta, rate, roll = 0, 0, 0
    clear_target(previous)
    previous = nil
end
local function reset()
    reset_motion()
    aiming = false
end
local function read_config()
    return ecs.is_alive(owner) and ecs.get(owner, "CameraController") or nil
end
-- Config edits use read_config to get a separate writable snapshot.
local function read_controller_snapshot()
    return ecs.is_alive(owner) and read_controller() or nil
end
local function controls_for(target)
    if not target then return nil end
    if target == ids.player and read_player_controls then return read_player_controls() end
    return ecs.get(target, "PlayerControls")
end
local function target_for(config)
    local target = config and ids[config.target]
    return target and ecs.is_alive(target) and ecs.has(target, "PlayerCamera") and target or nil
end
local function switch_target(target)
    if previous ~= target then
        clear_target(previous)
        previous, delta, rate, roll = target, 0, 0, 0
    end
end
ecs.on("input.reset", reset)
ecs.on("input.mouse_button", function(event)
    if event.button == "right" and (event.gameplay or not event.pressed) then aiming = event.pressed == true end
end)
ecs.on("input.mouse_delta", function(event)
    local config = read_controller_snapshot()
    local target = target_for(config)
    if not config or not config.enabled or not target then return end
    local controls = controls_for(target)
    if not controls or not controls.enabled or not controls.mouse_look then return end
    local dx, dy = tonumber(event.dx) or 0, tonumber(event.dy) or 0
    if dx ~= dx or dy ~= dy then return end
    dx, dy = math.max(-2000, math.min(2000, dx)), math.max(-2000, math.min(2000, dy))
    local camera = ecs.get(target, "PlayerCamera")
    local turn = dx * controls.mouse_sensitivity
    camera.yaw = (camera.yaw + turn + math.pi) % (2 * math.pi) - math.pi
    local vertical = controls.invert_y and 1 or -1
    local pitch_limit = math.rad(config.pitch_limit_degrees)
    camera.pitch = math.max(-pitch_limit, math.min(pitch_limit, camera.pitch + dy * controls.mouse_sensitivity * vertical))
    ecs.set_fields(target, "PlayerCamera", {yaw=camera.yaw, pitch=camera.pitch})
    switch_target(target)
    -- Neural-Sight's original yaw sign is opposite to CounterSplat's basis.
    delta = math.max(-10, math.min(10, delta - turn))
end)

local function sync()
    local config = read_config()
    if not config then return end
    local check = ids.camera_turn_enabled_ui and ecs.get(ids.camera_turn_enabled_ui, "UiElement")
    if check and check.widget.checked ~= config.turn_tilt_enabled then
        check.widget.checked = config.turn_tilt_enabled; ecs.set(ids.camera_turn_enabled_ui, "UiElement", check)
    end
    local slider = ids.camera_turn_strength_ui and ecs.get(ids.camera_turn_strength_ui, "UiElement")
    if slider then
        local value = math.max(slider.widget.min, math.min(slider.widget.max, config.turn_tilt_strength))
        if math.abs(slider.widget.value - value) > 0.0001 then
            slider.widget.value = value; ecs.set(ids.camera_turn_strength_ui, "UiElement", slider)
        end
    end
end
local function save(config)
    if pcall(function() ecs.set(owner, "CameraController", config) end) then
        ecs.emit("camera.controller_changed", config)
    end
    sync()
end
ecs.on("ui.action", function(action)
    if action.id == ids.camera_controller_reset and action.kind.type == "clicked" then
        save(ecs.get_resource("camera.default_controller") or initial)
        reset()
    elseif action.id == ids.camera_turn_enabled_ui and action.kind.type == "bool_changed" then
        local config = read_config()
        if config then config.turn_tilt_enabled = action.kind.value; save(config) end
    elseif action.id == ids.camera_turn_strength_ui and action.kind.type == "number_changed" then
        local config = read_config()
        if config then config.turn_tilt_strength = action.kind.value; save(config) end
    elseif action.id == ids.reset and action.kind.type == "clicked" then reset() end
end)
local ui_elapsed = 0
ecs.system("camera.neural_controller", function(dt)
    local config = read_controller_snapshot()
    local target = target_for(config)
    local controls = controls_for(target)
    if not config or not config.enabled or not config.turn_tilt_enabled or not target or
       not controls or not controls.enabled or not controls.mouse_look then
        reset_motion()
    else
        switch_target(target)
        local step = math.max(0, math.min(0.05, dt))
        if step > 0 then
            local speed = math.max(-math.pi * 2, math.min(math.pi * 2, delta / step))
            delta = 0
            rate = rate + (speed - rate) * (1 - math.exp(-step * config.turn_rate_response))
            local limit = math.rad(config.turn_tilt_max_degrees)
            local desired = math.max(-limit, math.min(limit, rate * 0.007 * config.turn_tilt_strength))
            if aiming then desired = desired * config.aim_tilt_scale end
            roll = roll + (desired - roll) * (1 - math.exp(-step * config.turn_tilt_response))
            if math.abs(rate) < 0.00002 and math.abs(roll) < 0.000002 then rate, roll = 0, 0 end
            local current_roll = ecs.get_field(target, "PlayerCamera", "view_turn_roll")
            -- Preserve custom cameras whose exposed object omits this field.
            if current_roll == nil then current_roll = ecs.get(target, "PlayerCamera").view_turn_roll end
            if current_roll ~= roll then
                ecs.set_field(target, "PlayerCamera", "view_turn_roll", roll)
            end
        end
    end
    ui_elapsed = ui_elapsed + math.max(0, dt)
    if ui_elapsed >= 0.25 then
        ui_elapsed = 0
        local panel = ids.camera_panel and ecs.get(ids.camera_panel, "UiElement")
        if panel and not panel.collapsible.collapsed then sync() end
    end
end)
sync()
