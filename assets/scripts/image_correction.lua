-- Scene-owned image correction. Rendering consumes the validated component;
-- this script owns the controls, exported tuning fields, and reset behavior.
local ids = assert(ecs.get_resource("scene.entities"))
local entity = assert(ids.image_correction)
assert(ecs.get(entity, "ImageCorrection"), "image correction is missing")

local definitions = {
    {"image_exposure", "exposure_ev", "Exposure EV", -8, 8},
    {"image_contrast", "contrast", "Contrast", 0.25, 2},
    {"image_saturation", "saturation", "Saturation", 0, 2},
    {"image_temperature", "temperature", "Warmth", -1, 1},
    {"image_tint", "tint", "Tint", -1, 1},
    {"image_shadows", "shadows", "Shadows", -1, 1},
    {"image_shadow_warmth", "shadow_warmth", "Shadow warmth", -1, 1},
    {"image_highlight_warmth", "highlight_warmth", "Highlight warmth", -1, 1},
    {"image_highlights", "highlights", "Highlights", -1, 1},
    {"image_bloom", "bloom_intensity", "Bloom strength", 0, 2},
    {"image_threshold", "bloom_threshold", "Bloom threshold", 0.01, 16},
    {"image_vignette", "vignette", "Vignette", 0, 1},
    {"image_grain", "grain", "Film grain", 0, 0.2},
    {"image_sharpness", "sharpness", "Sharpening", 0, 2},
}
local fields = {}
for _, item in ipairs(definitions) do
    fields[ids[item[1]]] = item[2]
    engine.export(entity, item[1], {component="ImageCorrection", field=item[2],
        label=item[3], min=item[4], max=item[5]})
end
local switches = {[ids.image_enabled]="enabled", [ids.image_filmic]="filmic"}
engine.export(entity, "image_enabled", {component="ImageCorrection", field="enabled", label="Enable grade"})
engine.export(entity, "image_filmic", {component="ImageCorrection", field="filmic", label="Filmic curve"})

local function sync()
    local grade = assert(ecs.get(entity, "ImageCorrection"))
    for id, field in pairs(fields) do
        local element = assert(ecs.get(id, "UiElement"))
        local widget = element.widget
        local value = math.max(widget.min, math.min(widget.max, grade[field]))
        if math.abs(widget.value - value) > 0.0001 then
            widget.value = value
            ecs.set(id, "UiElement", element)
        end
    end
    for id, field in pairs(switches) do
        local element = assert(ecs.get(id, "UiElement"))
        if element.widget.checked ~= grade[field] then
            element.widget.checked = grade[field]
            ecs.set(id, "UiElement", element)
        end
    end
end
local function save(grade)
    local ok = pcall(function() ecs.set(entity, "ImageCorrection", grade) end)
    if ok then ecs.emit("image.correction_changed", grade) end
    sync()
end
ecs.on("ui.action", function(action)
    local id, kind = action.id, action.kind
    if id == ids.image_reset and kind.type == "clicked" then
        save(assert(ecs.get_resource("image.default_correction")))
        return
    end
    local field = fields[id] or switches[id]
    if field and (kind.type == "number_changed" or kind.type == "bool_changed") then
        local grade = assert(ecs.get(entity, "ImageCorrection"))
        grade[field] = kind.value
        save(grade)
    end
end)
local elapsed = 0
sync()
ecs.system("image.correction_ui", function(dt)
    elapsed = elapsed + dt
    if elapsed < 0.25 then return end
    elapsed = 0
    local panel = ecs.get(ids.image_panel, "UiElement")
    if panel and not panel.collapsible.collapsed then sync() end
end)
