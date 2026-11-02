-- Right-button lens zoom is a Lua behavior. The base FOV stays editable and
-- zoom writes only the temporary render FOV of the player camera.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)
local weapon = ids.first_person
local optics = assert(engine.optics, "load weapon_optics.lua before camera_zoom.lua")
local read_controls = engine.component_reader(player, "PlayerControls")
local read_optic = weapon and engine.component_reader(weapon, "WeaponOptic")
local read_weapon_state = weapon and engine.component_reader(weapon, "WeaponState")
engine.export(player, "zoom_response", {component="PlayerControls", field="zoom_response", label="Zoom response", min=1, max=30})

local held = false
local magnification, lens_blend = 1, 0
local last_lens = nil
local last_standard, last_zoom, last_amount = nil, nil, nil
local read_standard_lens = engine.component_reader(player, "CameraLens")
local read_zoom_lens = engine.component_reader(player, "CameraZoomLens")
local lens_numbers = {"aperture_f_number", "shutter_angle_degrees", "shutter_fps", "iso",
    "exposure_compensation_ev", "focus_distance_m", "sensor_height_mm"}

-- The editable components stay independent. Publish only a transient render
-- snapshot, so entering or leaving zoom never overwrites either profile.
local function publish_lens(amount)
    local standard, zoom = read_standard_lens(), read_zoom_lens()
    if not standard or not zoom then
        last_standard, last_zoom, last_amount = nil, nil, nil
        if last_lens then ecs.remove_resource("camera.render_lens"); last_lens = nil end
        return
    end
    -- Readers return the same read-only table until a lens changes. Once the
    -- blend settles, neither interpolation nor signature allocation is needed.
    if standard == last_standard and zoom == last_zoom and amount == last_amount then return end
    local result, parts = {}, {}
    for _, field in ipairs(lens_numbers) do
        result[field] = standard[field] + (zoom[field] - standard[field]) * amount
        parts[#parts + 1] = tostring(result[field])
    end
    if amount >= 0.5 then
        result.depth_of_field, result.motion_blur = zoom.depth_of_field, zoom.motion_blur
    else
        result.depth_of_field, result.motion_blur = standard.depth_of_field, standard.motion_blur
    end
    parts[#parts + 1], parts[#parts + 2] = tostring(result.depth_of_field), tostring(result.motion_blur)
    local signature = table.concat(parts, ":")
    if signature ~= last_lens then
        ecs.set_resource("camera.render_lens", result)
        last_lens = signature
    end
    last_standard, last_zoom, last_amount = standard, zoom, amount
end

-- Scripted aiming and physical right-button aiming use the same behavior.
ecs.on("weapon.aim", function(event) held = event.pressed == true end)
ecs.on("input.mouse_button", function(event)
    if event.button ~= "right" then return end
    if event.pressed == false then held = false
    elseif event.pressed == true and event.gameplay == true then held = true end
end)

local function reset()
    held, magnification, lens_blend = false, 1, 0
    publish_lens(0)
    local camera = ecs.get(player, "PlayerCamera")
    if camera then ecs.set_field(player, "PlayerCamera", "view_fov_degrees", ecs.null) end
end
ecs.on("input.reset", reset)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then reset() end
end)

ecs.system("player.camera_zoom", function(dt)
    local current_view_fov, controls = ecs.get_field(player, "PlayerCamera", "view_fov_degrees"), read_controls()
    if current_view_fov == nil then
        -- A missing component still stops the system; a custom camera may
        -- expose an object without the optional field.
        local camera = ecs.get(player, "PlayerCamera")
        if not camera or not controls then return end
        current_view_fov = camera.view_fov_degrees
    elseif not controls then return end
    dt = math.max(0, math.min(dt, 0.25))
    if dt == 0 then return end
    local aiming = held and controls.enabled
    if aiming and read_weapon_state then
        local state = read_weapon_state()
        local name = type(state) == "table" and state.state or nil
        if name == "noGun" or name == "stow" or name == "equip" or name == "death" or
           name == "dead" or name == "reload" or name == "repack" then aiming = false end
    end
    local target = aiming and optics.magnification(read_optic and read_optic()) or 1
    local blend = 1 - math.exp(-controls.zoom_response * dt)
    magnification = magnification + (target - magnification) * blend
    if math.abs(magnification - target) < 0.0001 then magnification = target end
    local lens_target = aiming and 1 or 0
    lens_blend = lens_blend + (lens_target - lens_blend) * blend
    if math.abs(lens_blend - lens_target) < 0.0001 then lens_blend = lens_target end
    publish_lens(lens_blend)
    local view_fov = ecs.null
    if magnification ~= 1 then
        -- Magnification is proportional to focal length, not to FOV angle.
        local base_fov = ecs.get_field(player, "PlayerCamera", "fov_degrees")
        if base_fov == nil then base_fov = ecs.get(player, "PlayerCamera").fov_degrees end
        view_fov = math.deg(2 * math.atan(math.tan(math.rad(base_fov) / 2) / magnification))
    end
    if current_view_fov ~= view_fov then
        ecs.set_field(player, "PlayerCamera", "view_fov_degrees", view_fov)
    end
end)

publish_lens(0)
