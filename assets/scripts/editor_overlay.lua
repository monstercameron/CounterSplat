-- The developer panels are regular scene entities. Preserve their individual
-- visibility and collapse states while temporarily hiding the tools during play.
local ids = assert(ecs.get_resource("scene.entities"))
local tools = {}
for _, name in ipairs({"scene_panel", "sky_panel", "controls_panel", "level_panel",
    "entity_inspector", "ecs_panel", "camera_panel", "image_panel"}) do
    if ids[name] then tools[#tools + 1] = ids[name] end
end
local saved, showing = {}, false
local preferences = ecs.get_resource("hud.preferences") or {}
local show_performance = preferences.show_performance ~= false
local function visible(id, value)
    local element = id and ecs.get(id, "UiElement")
    if element and element.visible ~= value then
        element.visible = value
        ecs.set(id, "UiElement", element)
    end
end
local function set_tools(value)
    if showing == value then return end
    showing = value
    for _, id in ipairs(tools) do
        local element = ecs.get(id, "UiElement")
        if element then
            if showing then visible(id, saved[id] == true)
            else saved[id] = element.visible; visible(id, false) end
        end
    end
    visible(ids.stats_panel, showing or show_performance)
    ecs.set_resource("hud.editor_visible", showing)
    engine.events.emit("hud.editor_visibility_changed", {visible = showing})
end
-- Save scene-authored state on startup, including an initially closed inspector.
for _, id in ipairs(tools) do
    local element = ecs.get(id, "UiElement")
    if element then saved[id] = element.visible; visible(id, false) end
end
visible(ids.stats_panel, show_performance)
ecs.set_resource("hud.editor_visible", false)
ecs.on("hud.preferences_changed", function(value)
    if type(value) == "table" and type(value.show_performance) == "boolean" then
        show_performance = value.show_performance
        ecs.set_resource("hud.preferences", {show_performance = show_performance})
        visible(ids.stats_panel, showing or show_performance)
    end
end)
ecs.on("input.key", function(key)
    if key.code == "Backquote" and key.pressed and not key.is_repeat and key.gameplay and
        not ecs.get_resource("input.binding_capture") then
        set_tools(not showing)
    end
end)
