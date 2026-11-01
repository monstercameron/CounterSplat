-- The ECS entity browser and the right-hand inspector are ordinary scene UI.
-- Selection, physics controls, and Lua exports all use the standard ECS bridge.
local ids = assert(ecs.get_resource("scene.entities"))
local selected = assert(ids.player)
local page = 1
local page_size = 4
local slots = {ids.inspector_export_1, ids.inspector_export_2, ids.inspector_export_3, ids.inspector_export_4}
local labels = {}
for name, id in pairs(ids) do labels[id] = name end
local list_page, list_page_size, list_stamp = 1, 10, nil
local list_rows = {ids.ecs_row_1, ids.ecs_row_2, ids.ecs_row_3, ids.ecs_row_4, ids.ecs_row_5,
    ids.ecs_row_6, ids.ecs_row_7, ids.ecs_row_8, ids.ecs_row_9, ids.ecs_row_10}
local row_index, displayed = {}, {}
local has_list = ids.ecs_panel and ids.ecs_count and ids.ecs_page_prev and ids.ecs_page_label and ids.ecs_page_next
for index = 1, list_page_size do
    local id = list_rows[index]
    if id then row_index[id] = index else has_list = false end
end

local function display_name(id)
    local name = labels[id] or ("Entity " .. id)
    name = name:gsub("_", " "):gsub("^%l", string.upper)
    if #name > 31 then name = name:sub(1, 28) .. "..." end
    return name
end
local function sorted_entities()
    local entities, ui = ecs.entities(), {}
    for _, id in ipairs(ecs.query({"UiElement"})) do ui[id] = true end
    local rank = {}
    for _, id in ipairs(entities) do
        rank[id] = id == ids.player and 0 or (ui[id] and 2 or 1)
    end
    table.sort(entities, function(a, b)
        if rank[a] ~= rank[b] then return rank[a] < rank[b] end
        return (labels[a] or a) < (labels[b] or b)
    end)
    return entities
end
local function change(id, f)
    local element = assert(ecs.get(id, "UiElement"))
    f(element)
    ecs.set(id, "UiElement", element)
end
local function text(id, value) change(id, function(e) e.widget.text = value end) end
local function available(id, yes) change(id, function(e) e.enabled = yes end) end
local function check(id, value, yes)
    change(id, function(e) e.enabled = yes; e.widget.checked = value end)
end
local function slider(id, value, yes, minimum, maximum)
    change(id, function(e)
        e.enabled = yes
        if minimum then e.widget.min = minimum end
        if maximum then e.widget.max = maximum end
        e.widget.value = math.max(e.widget.min, math.min(e.widget.max, value))
    end)
end
-- Only rewrite rows when their contents change; the entity list stays live without
-- making a separate ECS request for every entity on every frame.
local function refresh_list(force)
    if not has_list then return end
    local entities = sorted_entities()
    local pages = math.max(1, math.ceil(#entities / list_page_size))
    list_page = math.max(1, math.min(list_page, pages))
    local stamp = table.concat(entities, "|") .. ":" .. list_page .. ":" .. tostring(selected)
    if not force and stamp == list_stamp then return end
    list_stamp = stamp
    text(ids.ecs_count, #entities .. (#entities == 1 and " entity" or " entities"))
    text(ids.ecs_page_label, list_page .. " / " .. pages)
    available(ids.ecs_page_prev, list_page > 1)
    available(ids.ecs_page_next, list_page < pages)
    for index, row in ipairs(list_rows) do
        local entity = entities[(list_page - 1) * list_page_size + index]
        displayed[index] = entity
        change(row, function(element)
            element.visible = entity ~= nil
            element.enabled = entity ~= nil
            if entity then
                element.widget.label = display_name(entity)
                local color = element.style.fill
                if entity == selected then color[1], color[2], color[3] = 46, 88, 85
                else color[1], color[2], color[3] = 21, 39, 52 end
            end
        end)
    end
end
local function publish_selection(entity)
    selected = entity
    page = 1
    if selected then
        ecs.set_resource("editor.selected_entity", selected)
        ecs.emit("editor.entity_selected", {entity=selected})
    else
        ecs.remove_resource("editor.selected_entity")
    end
end
local function open_selection(entity)
    if not entity or not ecs.is_alive(entity) then return end
    -- UI entities expose their ordinary visibility and input flags on demand.
    -- Keep the editor's own controls available so the browser cannot hide itself.
    local name = labels[entity] or ""
    if ecs.has(entity, "UiElement") and name ~= "entity_inspector" and
        not name:match("^ecs_") and not name:match("^inspector_") then
        engine.export(entity, "inspector_ui_visible", {component="UiElement", field="visible", label="Visible"})
        engine.export(entity, "inspector_ui_enabled", {component="UiElement", field="enabled", label="Interaction enabled"})
    end
    publish_selection(entity)
    change(ids.entity_inspector, function(element)
        element.visible = true
        element.collapsible.collapsed = false
    end)
end
local function component_lines(entity)
    local lines = {"Components: ", ""}
    for _, name in ipairs(ecs.components(entity)) do
        local line = #lines[1] + #name + 2 <= 43 and 1 or 2
        local prefix = (lines[line] == "" or lines[line] == "Components: ") and "" or ", "
        lines[line] = lines[line] .. prefix .. name
    end
    if lines[1] == "Components: " then lines[1] = "No components" end
    if #lines[2] > 43 then lines[2] = lines[2]:sub(1, 40) .. "..." end
    return lines[1], lines[2]
end
local function refresh()
    if not selected or not ecs.is_alive(selected) then
        publish_selection(ecs.is_alive(ids.player) and ids.player or nil)
    end
    if not selected then
        change(ids.entity_inspector, function(element) element.visible = false end)
        return
    end
    text(ids.inspector_entity, display_name(selected))
    local first, second = component_lines(selected)
    if ids.inspector_components_1 then text(ids.inspector_components_1, first) end
    if ids.inspector_components_2 then text(ids.inspector_components_2, second) end
    local body = ecs.get(selected, "PhysicsBody")
    local has_body = body ~= nil
    change(ids.inspector_attach, function(e) e.visible = not has_body end)
    check(ids.inspector_enabled, has_body and body.enabled or false, has_body)
    check(ids.inspector_collision, has_body and body.collision_enabled or false, has_body)
    check(ids.inspector_gravity, has_body and body.gravity_enabled or false, has_body)
    local rig = selected == ids.player and ecs.get(selected, "PlayerRig") or nil
    slider(ids.inspector_radius, has_body and body.radius or 0.2, has_body,
        rig and 0.1 * rig.units_per_meter or 0.05, rig and 0.33 * rig.units_per_meter or 1.0)
    slider(ids.inspector_gravity_strength, has_body and body.gravity or 18, has_body)
    slider(ids.inspector_offset, has_body and body.collision_offset_y or 0, has_body,
        rig and (rig.head_clearance_m - 2.5) * 0.5 * rig.units_per_meter or -8,
        rig and (rig.head_clearance_m - 0.7) * 0.5 * rig.units_per_meter or 8)
    local exports = engine.exports(selected)
    local pages = math.max(1, math.ceil(#exports / page_size))
    page = math.max(1, math.min(page, pages))
    for index, id in ipairs(slots) do
        local entry = exports[(page - 1) * page_size + index]
        change(id, function(element)
            element.visible = entry ~= nil
            element.enabled = false
            if entry then
                local target
                if entry.component then target = ecs.get(selected, entry.component)
                else target = ecs.get_resource(entry.resource) end
                local value = target and target[entry.field]
                if type(value) == entry.kind then
                    element.enabled = true
                    if entry.kind == "boolean" then
                        element.widget = {type="checkbox", label=entry.label, checked=value}
                    else
                        element.widget = {type="slider", label=entry.label, min=entry.min, max=entry.max,
                            value=math.max(entry.min, math.min(entry.max, value))}
                    end
                end
            end
        end)
    end
    text(ids.inspector_export_page, #exports == 0 and "No tunable fields" or ("Settings " .. page .. " / " .. pages))
    available(ids.inspector_export_prev, page > 1)
    available(ids.inspector_export_next, page < pages)
    if not has_body then text(ids.inspector_state, "No physics body")
    elseif not body.enabled then text(ids.inspector_state, "Physics disabled")
    elseif not body.collision_enabled then text(ids.inspector_state, "Collision disabled")
    else
        local status = ecs.get(ids.active_level, "VoxelColliderState")
        local level = ecs.get(ids.active_level, "VoxelCollider")
        if not level or not level.enabled or not level.physics then text(ids.inspector_state, "Level collision disabled")
        elseif not status or status.status ~= "ready" then text(ids.inspector_state, "Level collider unavailable")
        elseif body.gravity_enabled then text(ids.inspector_state, body.grounded and "Grounded" or "Airborne")
        else text(ids.inspector_state, "Level collider ready") end
    end
end
local function choose(direction)
    local entities = sorted_entities()
    if #entities == 0 then return end
    local current = 1
    for i, id in ipairs(entities) do if id == selected then current = i; break end end
    local index = ((current - 1 + direction) % #entities) + 1
    open_selection(entities[index])
    list_page = math.floor((index - 1) / list_page_size) + 1
end
local fields = {[ids.inspector_enabled]="enabled", [ids.inspector_collision]="collision_enabled",
    [ids.inspector_gravity]="gravity_enabled", [ids.inspector_radius]="radius", [ids.inspector_gravity_strength]="gravity",
    [ids.inspector_offset]="collision_offset_y"}

ecs.on("ui.action", function(action)
    local id, kind = action.id, action.kind
    if row_index[id] and kind.type == "clicked" then
        open_selection(displayed[row_index[id]])
    elseif id == ids.ecs_page_prev and kind.type == "clicked" then list_page = list_page - 1
    elseif id == ids.ecs_page_next and kind.type == "clicked" then list_page = list_page + 1
    elseif id == ids.inspector_close and kind.type == "clicked" then
        change(ids.entity_inspector, function(element) element.visible = false end)
    elseif id == ids.inspector_prev and kind.type == "clicked" then choose(-1)
    elseif id == ids.inspector_next and kind.type == "clicked" then choose(1)
    elseif id == ids.inspector_export_prev and kind.type == "clicked" then page = page - 1
    elseif id == ids.inspector_export_next and kind.type == "clicked" then page = page + 1
    elseif id == ids.inspector_attach and kind.type == "clicked" then
        if not ecs.has(selected, "PhysicsBody") then
            if not ecs.has(selected, "PlayerCamera") and not ecs.has(selected, "Transform") then
                ecs.set(selected, "Transform", {})
            end
            ecs.set(selected, "PhysicsBody", {})
        end
    elseif fields[id] and (kind.type == "bool_changed" or kind.type == "number_changed") then
        local body = ecs.get(selected, "PhysicsBody")
        if body then
            local field = fields[id]
            local previous_offset = body.collision_offset_y
            body[field] = kind.value
            local rig = selected == ids.player and ecs.get(selected, "PlayerRig") or nil
            if rig and field == "radius" then
                rig.radius_m = math.max(0.1, math.min(0.33, body.radius / rig.units_per_meter))
                body.radius = rig.radius_m * rig.units_per_meter
                body.capsule_half_height = ((rig.eye_height_m + rig.head_clearance_m) * 0.5 - rig.radius_m) * rig.units_per_meter
                body.collision_offset_y = (rig.head_clearance_m - rig.eye_height_m) * 0.5 * rig.units_per_meter
            elseif rig and field == "collision_offset_y" then
                local old_height = body.radius + body.capsule_half_height - previous_offset
                rig.eye_height_m = math.max(0.7, math.min(2.5, rig.head_clearance_m - 2 * body.collision_offset_y / rig.units_per_meter))
                body.capsule_half_height = ((rig.eye_height_m + rig.head_clearance_m) * 0.5 - rig.radius_m) * rig.units_per_meter
                body.collision_offset_y = (rig.head_clearance_m - rig.eye_height_m) * 0.5 * rig.units_per_meter
                local change = body.radius + body.capsule_half_height - body.collision_offset_y - old_height
                local camera = ecs.get(selected, "PlayerCamera")
                camera.position[2] = camera.position[2] + change
                ecs.set(selected, "PlayerCamera", camera)
                local spawn = ecs.get_resource("player.spawn_camera")
                if spawn then spawn.position[2] = spawn.position[2] + change; ecs.set_resource("player.spawn_camera", spawn) end
            end
            if (field == "enabled" or field == "collision_enabled") and not kind.value then body.grounded = false end
            if field == "gravity_enabled" and not kind.value then body.velocity[2] = 0; body.grounded = false end
            ecs.set(selected, "PhysicsBody", body)
            if rig and (field == "radius" or field == "collision_offset_y") then
                ecs.set(selected, "PlayerRig", rig)
                ecs.emit("player.rig_changed", rig)
            end
        end
    else
        for index, slot in ipairs(slots) do
            if id == slot and (kind.type == "bool_changed" or kind.type == "number_changed") then
                local entry = engine.exports(selected)[(page - 1) * page_size + index]
                if entry then
                    engine.set_export(selected, entry.name, kind.value)
                    if selected == ids.player and entry.component == "PlayerControls" then
                        ecs.emit("player.controls_changed", ecs.get(selected, "PlayerControls"))
                    elseif selected == ids.player and entry.component == "PlayerRig" then
                        ecs.emit("player.rig_changed", ecs.get(selected, "PlayerRig"))
                    end
                end
                break
            end
        end
    end
    refresh()
    refresh_list(false)
end)
local elapsed = 0
refresh()
refresh_list(true)
ecs.system("inspector.refresh", function(dt)
    elapsed = elapsed + dt
    if elapsed < 0.25 then return end
    elapsed = 0
    local panel = ecs.get(ids.entity_inspector, "UiElement")
    if selected and not ecs.is_alive(selected) then refresh()
    elseif panel and panel.visible and not panel.collapsible.collapsed then refresh() end
    local list = has_list and ecs.get(ids.ecs_panel, "UiElement") or nil
    if list and list.collapsible and not list.collapsible.collapsed then refresh_list(false) end
end)
