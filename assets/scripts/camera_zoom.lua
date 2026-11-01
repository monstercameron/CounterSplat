-- Right-button lens zoom is a Lua behavior. The base FOV stays editable and
-- zoom writes only the temporary render FOV of the player camera.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)
engine.export(player, "zoom_factor", {component="PlayerControls", field="zoom_factor", label="Zoom magnification", min=1, max=10})
engine.export(player, "zoom_response", {component="PlayerControls", field="zoom_response", label="Zoom response", min=1, max=30})

local held = false
local magnification, lens_blend = 1, 0
local last_lens = nil
local lens_numbers = {"aperture_f_number", "shutter_angle_degrees", "shutter_fps", "iso",
    "exposure_compensation_ev", "focus_distance_m", "sensor_height_mm"}

-- The editable components stay independent. Publish only a transient render
-- snapshot, so entering or leaving zoom never overwrites either profile.
local function publish_lens(amount)
    local standard, zoom = ecs.get(player, "CameraLens"), ecs.get(player, "CameraZoomLens")
    if not standard or not zoom then
        if last_lens then ecs.remove_resource("camera.render_lens"); last_lens = nil end
        return
    end
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
end

ecs.on("input.mouse_button", function(event)
    if event.button ~= "right" then return end
    if event.pressed == false then held = false
    elseif event.pressed == true and event.gameplay == true then held = true end
end)

local function reset()
    held, magnification, lens_blend = false, 1, 0
    publish_lens(0)
    local camera = ecs.get(player, "PlayerCamera")
    if camera then camera.view_fov_degrees = ecs.null; ecs.set(player, "PlayerCamera", camera) end
end
ecs.on("input.reset", reset)
ecs.on("ui.action", function(action)
    if action.id == ids.reset and action.kind.type == "clicked" then reset() end
end)

ecs.system("player.camera_zoom", function(dt)
    local camera = ecs.get(player, "PlayerCamera")
    local controls = ecs.get(player, "PlayerControls")
    if not camera or not controls then return end
    dt = math.max(0, math.min(dt, 0.25))
    if dt == 0 then return end
    local target = held and controls.enabled and controls.zoom_factor or 1
    local blend = 1 - math.exp(-controls.zoom_response * dt)
    magnification = magnification + (target - magnification) * blend
    if math.abs(magnification - target) < 0.0001 then magnification = target end
    local lens_target = held and controls.enabled and 1 or 0
    lens_blend = lens_blend + (lens_target - lens_blend) * blend
    if math.abs(lens_blend - lens_target) < 0.0001 then lens_blend = lens_target end
    publish_lens(lens_blend)
    local view_fov = ecs.null
    if magnification ~= 1 then
        -- Magnification is proportional to focal length, not to FOV angle.
        view_fov = math.deg(2 * math.atan(math.tan(math.rad(camera.fov_degrees) / 2) / magnification))
    end
    if camera.view_fov_degrees ~= view_fov then
        camera.view_fov_degrees = view_fov
        ecs.set(player, "PlayerCamera", camera)
    end
end)

publish_lens(0)
