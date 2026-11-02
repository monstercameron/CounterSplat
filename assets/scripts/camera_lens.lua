-- Camera behavior and UI bindings remain in Lua; the renderer reads CameraLens
-- and PlayerCamera through the same validated ECS adapters as every other system.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)
local weapon = ids.first_person
local optics = assert(engine.optics, "load weapon_optics.lua before camera_lens.lua")
assert(ecs.get(player, "CameraLens"), "camera lens is missing")
if not ecs.get(player, "CameraZoomLens") then ecs.set(player, "CameraZoomLens", ecs.get(player, "CameraLens")) end
local editing_zoom = false

engine.export(player, "lens_aperture", {component="CameraLens", field="aperture_f_number", label="Iris f/", min=1, max=32})
engine.export(player, "lens_shutter_angle", {component="CameraLens", field="shutter_angle_degrees", label="Shutter angle", min=1, max=360})
engine.export(player, "lens_shutter_fps", {component="CameraLens", field="shutter_fps", label="Shutter FPS", min=1, max=240})
engine.export(player, "lens_iso", {component="CameraLens", field="iso", label="ISO", min=25, max=6400})
engine.export(player, "lens_exposure", {component="CameraLens", field="exposure_compensation_ev", label="Exposure EV", min=-8, max=8})
engine.export(player, "lens_focus", {component="CameraLens", field="focus_distance_m", label="Focus distance (m)", min=0.1, max=1000})
engine.export(player, "lens_sensor", {component="CameraLens", field="sensor_height_mm", label="Sensor height (mm)", min=8, max=70})
engine.export(player, "lens_dof", {component="CameraLens", field="depth_of_field", label="Depth of field"})
engine.export(player, "lens_motion", {component="CameraLens", field="motion_blur", label="Motion blur"})
for _, entry in ipairs({
    {"zoom_lens_aperture","aperture_f_number","Zoom iris f/",1,32},
    {"zoom_lens_shutter_angle","shutter_angle_degrees","Zoom shutter angle",1,360},
    {"zoom_lens_shutter_fps","shutter_fps","Zoom shutter FPS",1,240},
    {"zoom_lens_iso","iso","Zoom ISO",25,6400},
    {"zoom_lens_exposure","exposure_compensation_ev","Zoom exposure EV",-8,8},
    {"zoom_lens_focus","focus_distance_m","Zoom focus distance (m)",0.1,1000},
    {"zoom_lens_sensor","sensor_height_mm","Zoom sensor height (mm)",8,70},
    {"zoom_lens_dof","depth_of_field","Zoom depth of field"},
    {"zoom_lens_motion","motion_blur","Zoom motion blur"},
}) do
    local spec = {component="CameraZoomLens", field=entry[2], label=entry[3]}
    if entry[4] then spec.min, spec.max = entry[4], entry[5] end
    engine.export(player, entry[1], spec)
end

local fields = { [ids.camera_aperture]="aperture_f_number", [ids.camera_shutter_angle]="shutter_angle_degrees",
    [ids.camera_shutter_fps]="shutter_fps", [ids.camera_iso]="iso", [ids.camera_ev]="exposure_compensation_ev",
    [ids.camera_focus]="focus_distance_m", [ids.camera_sensor]="sensor_height_mm" }
local switches = { [ids.camera_dof]="depth_of_field", [ids.camera_motion_blur]="motion_blur" }
local angles = { [ids.camera_yaw]="yaw", [ids.camera_pitch]="pitch", [ids.camera_roll]="roll" }

local function slider(id, value, minimum, maximum)
    local element = assert(ecs.get(id, "UiElement"))
    local widget = element.widget
    local lo, hi = minimum or widget.min, maximum or widget.max
    value = math.max(lo, math.min(hi, value))
    if widget.min ~= lo or widget.max ~= hi or math.abs(widget.value - value) > 0.0001 then
        widget.min, widget.max, widget.value = lo, hi, value
        ecs.set(id, "UiElement", element)
    end
end
local function check(id, value)
    local element = assert(ecs.get(id, "UiElement"))
    if element.widget.checked ~= value then element.widget.checked = value; ecs.set(id, "UiElement", element) end
end
local function profile_name()
    return editing_zoom and "CameraZoomLens" or "CameraLens"
end
local function show(id, visible)
    local element = assert(ecs.get(id, "UiElement"))
    if element.visible ~= visible or element.enabled ~= visible then
        element.visible, element.enabled = visible, visible
        ecs.set(id, "UiElement", element)
    end
end
local function label(id, value, field)
    local element = assert(ecs.get(id, "UiElement"))
    field = field or "text"
    if element.widget[field] ~= value then element.widget[field] = value; ecs.set(id, "UiElement", element) end
end
local function sync()
    local camera = assert(ecs.get(player, "PlayerCamera"))
    local controls = assert(ecs.get(player, "PlayerControls"))
    local current = assert(ecs.get(player, profile_name()))
    local optic = weapon and ecs.get(weapon, "WeaponOptic") or nil
    local kind = optic and optic.kind or "iron"
    local magnification = optics.magnification(optic)
    local names = {iron="Iron sights", red_dot="Red dot", scope="Scope"}
    local title = names[kind] or names.iron
    show(ids.camera_fov, not editing_zoom)
    show(ids.camera_zoom_factor, editing_zoom and kind == "scope")
    show(ids.camera_zoom_info, editing_zoom and kind ~= "scope")
    label(ids.camera_optic, string.format("Optic: %s · %.3g×", title, magnification), "label")
    label(ids.camera_zoom_info, string.format("%s · %.3g× ADS", title, magnification))
    check(ids.camera_lens_zoom, editing_zoom)
    label(ids.camera_exposure_title, editing_zoom and "Zoom lens" or "Standard lens")
    label(ids.camera_reset, editing_zoom and "Reset zoom lens" or "Reset standard lens", "label")
    slider(ids.camera_fov, camera.fov_degrees)
    if optic then slider(ids.camera_zoom_factor, optic.scope_magnification) end
    for id, field in pairs(angles) do slider(id, math.deg(camera[field])) end
    for id, field in pairs(fields) do slider(id, current[field]) end
    for id, field in pairs(switches) do check(id, current[field]) end
    local max_ms = 1000 / current.shutter_fps
    local time_ms = max_ms * current.shutter_angle_degrees / 360
    slider(ids.camera_shutter_ms, time_ms, max_ms / 360, max_ms)
    local label = assert(ecs.get(ids.camera_shutter_readout, "UiElement"))
    local text = string.format("1/%.0f s  |  %.0f deg at %.0f fps", 1000 / time_ms,
        current.shutter_angle_degrees, current.shutter_fps)
    if label.widget.text ~= text then label.widget.text = text; ecs.set(ids.camera_shutter_readout, "UiElement", label) end
end

local function save_lens(value)
    local component = profile_name()
    local ok = pcall(function() ecs.set(player, component, value) end)
    if ok then ecs.emit(editing_zoom and "camera.zoom_lens_changed" or "camera.lens_changed", value) end
    sync()
end

ecs.on("ui.action", function(action)
    local id, kind = action.id, action.kind
    if id == ids.camera_optic and kind.type == "clicked" and weapon then
        local current = ecs.get(weapon, "WeaponOptic") or {kind="iron", scope_magnification=4}
        local next_kind = current.kind == "iron" and "red_dot" or current.kind == "red_dot" and "scope" or "iron"
        optics.set(weapon, {kind=next_kind})
        sync()
        return
    end
    if id == ids.camera_lens_zoom and kind.type == "bool_changed" then
        editing_zoom = kind.value == true
        sync()
        return
    end
    if id == ids.camera_reset and kind.type == "clicked" then
        save_lens(assert(ecs.get_resource(editing_zoom and "camera.default_zoom_lens" or "camera.default_lens")))
        return
    end
    if id == ids.camera_zoom_factor and kind.type == "number_changed" then
        if weapon then optics.set(weapon, {scope_magnification=kind.value}) end
        sync()
        return
    end
    local field = fields[id]
    if field and kind.type == "number_changed" then
        local current = assert(ecs.get(player, profile_name()))
        current[field] = kind.value
        save_lens(current)
        return
    end
    field = switches[id]
    if field and kind.type == "bool_changed" then
        local current = assert(ecs.get(player, profile_name()))
        current[field] = kind.value
        save_lens(current)
        return
    end
    if id == ids.camera_shutter_ms and kind.type == "number_changed" then
        local current = assert(ecs.get(player, profile_name()))
        current.shutter_angle_degrees = math.max(1, math.min(360, kind.value * current.shutter_fps * 360 / 1000))
        save_lens(current)
        return
    end
    field = angles[id]
    if field and kind.type == "number_changed" then
        local camera = assert(ecs.get(player, "PlayerCamera"))
        local value = math.rad(kind.value)
        if field == "pitch" then value = math.max(-1.553343, math.min(1.553343, value)) end
        camera[field] = value
        pcall(function() ecs.set(player, "PlayerCamera", camera) end)
        sync()
        return
    end
    if id == ids.camera_fov and kind.type == "number_changed" then
        local camera = assert(ecs.get(player, "PlayerCamera"))
        local controls = assert(ecs.get(player, "PlayerControls"))
        camera.fov_degrees, controls.fov_degrees = kind.value, kind.value
        if pcall(function() ecs.set(player, "PlayerControls", controls); ecs.set(player, "PlayerCamera", camera) end) then
            ecs.emit("player.controls_changed", controls)
        end
        sync()
    end
end)
local elapsed = 0
sync()
ecs.system("camera.lens_ui", function(dt)
    elapsed = elapsed + dt
    if elapsed < 0.25 then return end
    elapsed = 0
    local panel = ecs.get(ids.camera_panel, "UiElement")
    if panel and not panel.collapsible.collapsed then sync() end
end)
