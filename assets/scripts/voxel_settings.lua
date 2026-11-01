-- Bind the voxel controls to the active level's normal ECS components.
local ids = assert(ecs.get_resource("scene.entities"))
local level = assert(ids.active_level)
local fields = { [ids.voxel_enabled] = "enabled", [ids.voxel_visible] = "visible", [ids.voxel_physics] = "physics" }

local function sync()
    local settings = assert(ecs.get(level, "VoxelCollider"))
    local state = assert(ecs.get(level, "VoxelColliderState"))
    local available = state.status == "ready" or state.status == "loading"
    for id, field in pairs(fields) do
        local element = assert(ecs.get(id, "UiElement"))
        local enabled = available and (field == "enabled" or settings.enabled)
        if element.widget.checked ~= settings[field] or element.enabled ~= enabled then
            element.widget.checked = settings[field]; element.enabled = enabled
            ecs.set(id, "UiElement", element)
        end
    end
    local text
    if state.status == "loading" then text = "Loading voxel collider..."
    elseif state.status == "unavailable" then text = "No voxel collider for this level"
    elseif state.status == "error" then text = "Voxel collider unavailable"
    elseif not settings.enabled then text = "Voxel system off"
    elseif settings.visible then text = string.format("%d nearby voxels", state.visible_voxels)
    else text = string.format("Voxel size: %.3f", state.resolution) end
    local label = assert(ecs.get(ids.voxel_status, "UiElement"))
    if label.widget.text ~= text then label.widget.text = text; ecs.set(ids.voxel_status, "UiElement", label) end
end
sync()
ecs.on("ui.action", function(action)
    local field = fields[action.id]
    if not field or action.kind.type ~= "bool_changed" then return end
    local settings = assert(ecs.get(level, "VoxelCollider"))
    settings[field] = action.kind.value
    ecs.set(level, "VoxelCollider", settings)
    sync()
end)
ecs.system("voxel.settings_ui", function() sync() end)
