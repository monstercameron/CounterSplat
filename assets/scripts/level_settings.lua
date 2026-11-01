-- The level inventory and LOD settings are normal, validated ECS components.
-- The renderer consumes LevelSettings; this script only binds the canvas controls.
local ids = assert(ecs.get_resource("scene.entities"), "gameplay entities are missing")
local level = assert(ids.active_level, "active level entity is missing")
local assets = assert(ecs.get(level, "LevelAssets"), "level assets are missing")
local lod_count = assets.lod_count
local last_manual = 0

local function set_text(id, value)
    local element = assert(ecs.get(id, "UiElement"))
    element.widget.text = value
    ecs.set(id, "UiElement", element)
end
set_text(ids.level_name, assets.name)
set_text(ids.level_summary, string.format("%d assets  ·  %d LOD%s", #assets.files, lod_count, lod_count == 1 and "" or "s"))
set_text(ids.level_hint, lod_count == 1 and "This scan has one LOD" or "0 = finest; higher = coarser")

local function sync_settings()
    local settings = assert(ecs.get(level, "LevelSettings"), "level settings are missing")
    local automatic = settings.lod == ecs.null
    if not automatic then last_manual = settings.lod end
    local slider = assert(ecs.get(ids.level_lod, "UiElement"))
    local max_lod = math.max(1, lod_count - 1)
    local value = math.max(0, math.min(max_lod, last_manual))
    local enabled = lod_count > 1
    if slider.widget.min ~= 0 or slider.widget.max ~= max_lod or slider.widget.value ~= value or slider.enabled ~= enabled then
        slider.widget.min = 0; slider.widget.max = max_lod; slider.widget.value = value; slider.enabled = enabled
        ecs.set(ids.level_lod, "UiElement", slider)
    end
    local distance = assert(ecs.get(ids.level_draw_distance, "UiElement"))
    if distance.widget.value ~= settings.draw_distance then
        distance.widget.value = settings.draw_distance
        ecs.set(ids.level_draw_distance, "UiElement", distance)
    end
    local checkbox = assert(ecs.get(ids.level_auto, "UiElement"))
    if checkbox.widget.checked ~= automatic or checkbox.enabled ~= enabled then
        checkbox.widget.checked = automatic; checkbox.enabled = enabled
        ecs.set(ids.level_auto, "UiElement", checkbox)
    end
end
sync_settings()

ecs.on("ui.action", function(action)
    local settings = assert(ecs.get(level, "LevelSettings"))
    if action.id == ids.level_lod and action.kind.type == "number_changed" and lod_count > 1 then
        last_manual = math.max(0, math.min(lod_count - 1, math.floor(action.kind.value + 0.5)))
        settings.lod = last_manual
    elseif action.id == ids.level_auto and action.kind.type == "bool_changed" and lod_count > 1 then
        settings.lod = action.kind.value and ecs.null or last_manual
    elseif action.id == ids.level_draw_distance and action.kind.type == "number_changed" then
        settings.draw_distance = math.max(10, math.min(1000, action.kind.value))
    else
        return
    end
    ecs.set(level, "LevelSettings", settings)
    sync_settings()
end)

-- External scripts can edit the same component; keep the panel in sync with those edits.
ecs.system("level.settings_ui", function() sync_settings() end)
