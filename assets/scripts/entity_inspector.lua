-- The ECS entity browser and the right-hand inspector are ordinary scene UI.
-- Selection, physics controls, and Lua exports all use the standard ECS bridge.
local ids = assert(ecs.get_resource("scene.entities"))
local selected = assert(ids.player)
local page = 1
local page_size = 4
local slots = {ids.inspector_export_1, ids.inspector_export_2, ids.inspector_export_3, ids.inspector_export_4}
local labels = {}
for name, id in pairs(ids) do labels[id] = name end
local list_stamp, geometry_stamp, row_height = nil, nil, 31
local row_stamps, count_stamp, controls_stamp = {}, nil, nil
local list_rows = {ids.ecs_row_1, ids.ecs_row_2, ids.ecs_row_3, ids.ecs_row_4, ids.ecs_row_5,
    ids.ecs_row_6, ids.ecs_row_7, ids.ecs_row_8, ids.ecs_row_9, ids.ecs_row_10,
    ids.ecs_row_11, ids.ecs_row_12}
local toggles, row_index, toggle_index, displayed = {}, {}, {}, {}
local internal, expanded = {}, {}
-- Scenes may choose which authored branches start open; runtime expansion is local UI state.
local preview = ecs.get_resource("preview") or {}
if type(preview.entity_browser_expanded) == "table" then
    for _, name in ipairs(preview.entity_browser_expanded) do
        if type(name) == "string" and ids[name] then expanded[ids[name]] = true end
    end
end
local parenting, notice, reveal = nil, nil, nil
local has_list = ids.ecs_panel and ids.ecs_count and ids.ecs_list
for index = 1, 12 do
    local id = list_rows[index]
    if id then row_index[id] = index else has_list = false end
end

local function display_name(id)
    local name = labels[id] or ("Entity " .. id)
    name = name:gsub("_", " "):gsub("%f[%a]ui%f[%A]", "UI"):gsub("%f[%a]ecs%f[%A]", "ECS"):gsub("^%l", string.upper)
    if #name > 31 then name = name:sub(1, 28) .. "..." end
    return name
end
local function protected(id)
    local name = labels[id] or ""
    return internal[id] or name == "entity_inspector" or name:match("^ecs_") or name:match("^inspector_")
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

-- Recycle enough row controls for the current viewport. These UI-only helper
-- entities are excluded from the tree so resizing cannot grow the tree itself.
local function make_control(template)
    local id = ecs.spawn()
    internal[id] = true
    local element = assert(ecs.get(template, "UiElement"))
    element.visible, element.enabled, element.draggable = false, false, false
    ecs.set(id, "UiElement", element)
    ecs.set_parent(id, ids.ecs_list)
    return id
end
local function ensure_rows(count)
    while #list_rows < count do
        local row = make_control(ids.ecs_row_1)
        list_rows[#list_rows + 1] = row
        row_index[row] = #list_rows
    end
    for index = 1, #list_rows do
        if not toggles[index] then
            local toggle = make_control(ids.ecs_row_1)
            toggle_index[toggle] = index
            toggles[index] = toggle
        end
    end
end

-- A compact ECS hierarchy snapshot keeps this live without copying component
-- payloads or making one host round-trip per entity.
local function entity_tree()
    local hierarchy, ui, by_id, children, roots = ecs.hierarchy(), {}, {}, {}, {}
    for _, id in ipairs(ecs.query({"UiElement"})) do ui[id] = true end
    for _, item in ipairs(hierarchy) do
        if not internal[item.id] then by_id[item.id] = item end
    end
    local function less(a, b)
        local ar = a == ids.player and 0 or (ui[a] and 2 or 1)
        local br = b == ids.player and 0 or (ui[b] and 2 or 1)
        if ar ~= br then return ar < br end
        local an, bn = labels[a] or a, labels[b] or b
        if an ~= bn then return an < bn end
        return a < b
    end
    local total = 0
    for id, item in pairs(by_id) do
        total = total + 1
        local parent = type(item.parent) == "string" and item.parent or nil
        if parent and by_id[parent] then
            children[parent] = children[parent] or {}
            table.insert(children[parent], id)
        else
            table.insert(roots, id)
        end
    end
    table.sort(roots, less)
    for _, siblings in pairs(children) do table.sort(siblings, less) end
    local visible, stack = {}, {}
    for i = #roots, 1, -1 do stack[#stack + 1] = {id=roots[i], depth=0} end
    while #stack > 0 do
        local item = table.remove(stack)
        local descendants = children[item.id]
        item.has_children = descendants ~= nil and #descendants > 0
        visible[#visible + 1] = item
        if item.has_children and expanded[item.id] then
            for i = #descendants, 1, -1 do
                stack[#stack + 1] = {id=descendants[i], depth=item.depth + 1}
            end
        end
    end
    return visible, total
end
local function sorted_entities()
    local entries, result = entity_tree(), {}
    for _, entry in ipairs(entries) do result[#result + 1] = entry.id end
    return result
end

-- Keep the toolbar and clipped list fitted to the panel after interactive resize,
-- restored browser preferences, or a reset of the user's UI layout.
local function sync_geometry()
    local panel = ecs.get(ids.ecs_panel, "UiElement")
    local viewport = ecs.get(ids.ecs_list, "UiElement")
    if not panel or not viewport or viewport.widget.type ~= "scroll_area" then return nil end
    local stamp = panel.rect.width .. ":" .. panel.rect.height
    if stamp ~= geometry_stamp then
        geometry_stamp = stamp
        local width = panel.rect.width - 24
        local height = panel.rect.height - panel.collapsible.header_height - 62 - 14
        change(ids.ecs_count, function(e) e.rect.width = width end)
        if ids.ecs_parent and ids.ecs_root then
            local button_width = (width - 8) * 0.5
            change(ids.ecs_parent, function(e) e.rect.width = button_width end)
            change(ids.ecs_root, function(e) e.rect.x = 20 + button_width; e.rect.width = button_width end)
        end
        viewport.rect.width, viewport.rect.height = width, height
        viewport.widget.content_height = math.max(height, viewport.widget.content_height)
        viewport.widget.offset = math.max(0, math.min(viewport.widget.offset, viewport.widget.content_height - height))
        ecs.set(ids.ecs_list, "UiElement", viewport)
    end
    ensure_rows(math.ceil(viewport.rect.height / row_height) + 1)
    return viewport
end

local function refresh_list(force)
    if not has_list then return end
    local viewport = sync_geometry()
    if not viewport then return end
    local entries, total = entity_tree()
    local content_height = math.max(viewport.rect.height, #entries * row_height)
    local offset = math.max(0, math.min(viewport.widget.offset, content_height - viewport.rect.height))
    if reveal then
        for index, entry in ipairs(entries) do
            if entry.id == reveal then
                local top, bottom = (index - 1) * row_height, (index - 1) * row_height + 27
                if top < offset then offset = top
                elseif bottom > offset + viewport.rect.height then offset = bottom - viewport.rect.height end
                break
            end
        end
        offset = math.max(0, math.min(offset, content_height - viewport.rect.height))
        reveal = nil
    end
    if viewport.widget.content_height ~= content_height or viewport.widget.offset ~= offset then
        viewport.widget.content_height, viewport.widget.offset = content_height, offset
        ecs.set(ids.ecs_list, "UiElement", viewport)
    end
    local keys = {}
    for _, entry in ipairs(entries) do
        keys[#keys + 1] = entry.id .. ":" .. entry.depth .. ":" .. tostring(entry.has_children) .. ":" .. tostring(expanded[entry.id])
    end
    local first = math.floor(offset / row_height)
    local stamp = table.concat(keys, "|") .. ":" .. total .. ":" .. offset .. ":" .. viewport.rect.width .. ":" ..
        tostring(selected) .. ":" .. tostring(parenting) .. ":" .. tostring(notice)
    if not force and stamp == list_stamp then return end
    list_stamp = stamp
    local caption = notice or (parenting and ("Choose parent for " .. display_name(parenting))) or
        (total .. (total == 1 and " entity" or " entities"))
    if force or caption ~= count_stamp then text(ids.ecs_count, caption); count_stamp = caption end
    if ids.ecs_parent and ids.ecs_root then
        local can_edit = selected and ecs.is_alive(selected) and not protected(selected)
        local parent_enabled = parenting ~= nil or not not can_edit
        local root_enabled = not parenting and not not can_edit and ecs.parent(selected) ~= nil
        local controls = tostring(parent_enabled) .. ":" .. tostring(root_enabled) .. ":" .. tostring(parenting ~= nil)
        if force or controls ~= controls_stamp then
            change(ids.ecs_parent, function(e)
                e.enabled = parent_enabled
                e.widget.label = parenting and "Cancel" or "Set parent"
            end)
            change(ids.ecs_root, function(e) e.enabled = root_enabled end)
            controls_stamp = controls
        end
    end
    for index, row in ipairs(list_rows) do
        local entry = entries[first + index]
        displayed[index] = entry
        local indent = entry and math.min(entry.depth, 8) * 14 or 0
        local y = (first + index - 1) * row_height
        local row_stamp = entry and table.concat({entry.id, indent, y, viewport.rect.width,
            tostring(entry.id == selected), tostring(entry.has_children), tostring(expanded[entry.id])}, ":") or "empty"
        if force or row_stamp ~= row_stamps[index] then
            change(row, function(element)
                element.rect.x, element.rect.y = indent + 21, y
                element.rect.width = math.max(32, viewport.rect.width - indent - 35)
                element.visible, element.enabled = entry ~= nil, entry ~= nil
                if entry then
                    element.widget.label = display_name(entry.id)
                    local color = element.style.fill
                    if entry.id == selected then color[1], color[2], color[3] = 46, 88, 85
                    else color[1], color[2], color[3] = 21, 39, 52 end
                end
            end)
            change(toggles[index], function(element)
                element.rect.x, element.rect.y, element.rect.width = indent, y, 19
                element.rect.height, element.z = 27, 103
                element.visible, element.enabled = entry ~= nil and entry.has_children, entry ~= nil and entry.has_children
                if entry then element.widget.label = expanded[entry.id] and "-" or "+" end
            end)
            row_stamps[index] = row_stamp
        end
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
    notice = nil
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
    -- Keep keyboard/inspector navigation visible in the same scrolling list.
    if has_list then
        local viewport = ecs.get(ids.ecs_list, "UiElement")
        if viewport and viewport.widget.type == "scroll_area" then
            local top, bottom = (index - 1) * row_height, (index - 1) * row_height + 27
            local offset = viewport.widget.offset
            if top < offset then offset = top
            elseif bottom > offset + viewport.rect.height then offset = bottom - viewport.rect.height end
            viewport.widget.offset = math.max(0, math.min(offset, viewport.widget.content_height - viewport.rect.height))
            ecs.set(ids.ecs_list, "UiElement", viewport)
        end
    end
end
local fields = {[ids.inspector_enabled]="enabled", [ids.inspector_collision]="collision_enabled",
    [ids.inspector_gravity]="gravity_enabled", [ids.inspector_radius]="radius", [ids.inspector_gravity_strength]="gravity",
    [ids.inspector_offset]="collision_offset_y"}

ecs.on("ui.action", function(action)
    local id, kind = action.id, action.kind
    if toggle_index[id] and kind.type == "clicked" then
        local entry = displayed[toggle_index[id]]
        if entry and entry.has_children then expanded[entry.id] = not expanded[entry.id] end
    elseif id == ids.ecs_parent and kind.type == "clicked" then
        if parenting then parenting, notice = nil, nil
        elseif selected and not protected(selected) then parenting, notice = selected, nil end
    elseif id == ids.ecs_root and kind.type == "clicked" then
        if selected and ecs.is_alive(selected) and not protected(selected) then
            local ok = pcall(function() ecs.set_parent(selected, nil) end)
            notice = ok and ("Made root: " .. display_name(selected)) or "Could not clear parent"
            reveal = selected
        end
    elseif row_index[id] and kind.type == "clicked" then
        local entry = displayed[row_index[id]]
        local target = entry and entry.id
        if parenting and target then
            if not ecs.is_alive(parenting) then
                parenting, notice = nil, "Entity no longer exists"
            elseif protected(target) then
                notice = "Choose a non-editor parent"
            elseif target == parenting then
                notice = "An entity cannot own itself"
            else
                local child = parenting
                local ok = pcall(function() ecs.set_parent(child, target) end)
                if ok then
                    expanded[target] = true
                    local ancestor = ecs.parent(target)
                    while ancestor do expanded[ancestor] = true; ancestor = ecs.parent(ancestor) end
                    parenting, notice, reveal = nil, "Parent set: " .. display_name(child), child
                else
                    notice = "Cannot parent: cycle or missing entity"
                end
            end
        elseif target then open_selection(target) end
    elseif id == ids.ecs_list and kind.type == "scroll_changed" then
        -- The UI adapter has already applied the offset; refresh the recycled rows.
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
                    elseif selected == ids.player and entry.component == "CameraLens" then
                        ecs.emit("camera.lens_changed", ecs.get(selected, "CameraLens"))
                    elseif selected == ids.player and entry.component == "CameraZoomLens" then
                        ecs.emit("camera.zoom_lens_changed", ecs.get(selected, "CameraZoomLens"))
                    elseif selected == ids.image_correction and entry.component == "ImageCorrection" then
                        ecs.emit("image.correction_changed", ecs.get(selected, "ImageCorrection"))
                    elseif selected == ids.camera_controller and entry.component == "CameraController" then
                        ecs.emit("camera.controller_changed", ecs.get(selected, "CameraController"))
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
