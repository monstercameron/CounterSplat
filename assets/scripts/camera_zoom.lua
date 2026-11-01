-- Right-button lens zoom is a Lua behavior. The base FOV stays editable and
-- zoom writes only the temporary render FOV of the player camera.
local ids = assert(ecs.get_resource("scene.entities"))
local player = assert(ids.player)
engine.export(player, "zoom_factor", {component="PlayerControls", field="zoom_factor", label="Zoom magnification", min=1, max=10})
engine.export(player, "zoom_response", {component="PlayerControls", field="zoom_response", label="Zoom response", min=1, max=30})

local held = false
local magnification = 1

ecs.on("input.mouse_button", function(event)
    if event.button ~= "right" then return end
    if event.pressed == false then held = false
    elseif event.pressed == true and event.gameplay == true then held = true end
end)

local function reset()
    held, magnification = false, 1
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
    if magnification == 1 then
        camera.view_fov_degrees = ecs.null
    else
        -- Magnification is proportional to focal length, not to FOV angle.
        camera.view_fov_degrees = math.deg(2 * math.atan(math.tan(math.rad(camera.fov_degrees) / 2) / magnification))
    end
    ecs.set(player, "PlayerCamera", camera)
end)
