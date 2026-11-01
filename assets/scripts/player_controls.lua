-- The player, UI, and settings are ordinary ECS components. The host supplies
-- ordered input events and runs this system at the engine's fixed simulation tick.
local ids = assert(ecs.get_resource("scene.entities"), "gameplay entities are missing")
local player = assert(ids.player, "player entity is missing")
local held = {}
local waiting_for_key = nil
local sliders = { [ids.controls_speed] = "move_speed", [ids.controls_sprint] = "sprint_multiplier", [ids.controls_sensitivity] = "mouse_sensitivity", [ids.controls_fov] = "fov_degrees" }
local checks = { [ids.controls_enabled] = "enabled", [ids.controls_mouse] = "mouse_look", [ids.controls_invert] = "invert_y" }
local keys = { [ids.key_forward] = "forward", [ids.key_backward] = "backward", [ids.key_left] = "left", [ids.key_right] = "right", [ids.key_up] = "up", [ids.key_down] = "down", [ids.key_sprint] = "sprint" }

local function notice(text)
    local element = ecs.get(ids.controls_notice, "UiElement")
    if element then element.widget.text = text; ecs.set(ids.controls_notice, "UiElement", element) end
end
local function key_label(code)
    if code:match("^Key%u$") then return code:sub(4) end
    if code:match("^Digit%d$") then return code:sub(6) end
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
    for id, field in pairs(checks) do
        local element = ecs.get(id, "UiElement"); element.widget.checked = controls[field]; ecs.set(id, "UiElement", element)
    end
    for id, field in pairs(keys) do
        local element = ecs.get(id, "UiElement"); element.widget.label = key_label(controls.bindings[field]); ecs.set(id, "UiElement", element)
    end
end
local initial = assert(ecs.get(player, "PlayerControls"), "player settings are missing")
if initial.fov_degrees ~= ecs.null then
    local camera = ecs.get(player, "PlayerCamera"); camera.fov_degrees = initial.fov_degrees; ecs.set(player, "PlayerCamera", camera)
end
sync_controls(initial)

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
    if not event.pressed then held[event.code] = nil
    elseif event.gameplay and not event.is_repeat then held[event.code] = true end
end)
ecs.on("input.mouse_delta", function(event)
    local settings = ecs.get(player, "PlayerControls")
    if not settings or not settings.enabled or not settings.mouse_look then return end
    local camera = ecs.get(player, "PlayerCamera")
    local dx = math.max(-2000, math.min(2000, event.dx or 0))
    local dy = math.max(-2000, math.min(2000, event.dy or 0))
    camera.yaw = (camera.yaw + dx * settings.mouse_sensitivity + math.pi) % (2 * math.pi) - math.pi
    local vertical = settings.invert_y and 1 or -1
    camera.pitch = math.max(-1.553343, math.min(1.553343, camera.pitch + dy * settings.mouse_sensitivity * vertical))
    ecs.set(player, "PlayerCamera", camera)
end)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then
        local spawn = ecs.get_resource("player.spawn_camera")
        local settings = ecs.get(player, "PlayerControls")
        if spawn then
            if settings.fov_degrees ~= ecs.null then spawn.fov_degrees = settings.fov_degrees end
            ecs.set(player, "PlayerCamera", spawn)
        end
        held = {}
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
        local camera = ecs.get(player, "PlayerCamera")
        camera.fov_degrees = ecs.get_resource("player.spawn_camera").fov_degrees
        ecs.set(player, "PlayerCamera", camera)
        sync_controls(controls); notice("Defaults restored"); held = {}
        ecs.emit("player.controls_changed", controls)
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
    local settings = ecs.get(player, "PlayerControls")
    if not settings or not settings.enabled then return end
    local b = settings.bindings
    local function down(code) return held[code] and 1 or 0 end
    local forward = down(b.forward) - down(b.backward)
    local right = down(b.right) - down(b.left)
    local up = down(b.up) - down(b.down)
    local length = math.sqrt(forward * forward + right * right + up * up)
    if length == 0 then return end
    local camera = ecs.get(player, "PlayerCamera")
    local speed = settings.move_speed * (held[b.sprint] and settings.sprint_multiplier or 1) * dt / length
    local sine, cosine = math.sin(camera.yaw), math.cos(camera.yaw)
    camera.position[1] = math.max(-10000000, math.min(10000000, camera.position[1] + (forward * sine + right * cosine) * speed))
    camera.position[2] = math.max(-10000000, math.min(10000000, camera.position[2] + up * speed))
    camera.position[3] = math.max(-10000000, math.min(10000000, camera.position[3] + (-forward * cosine + right * sine) * speed))
    ecs.set(player, "PlayerCamera", camera)
end)
