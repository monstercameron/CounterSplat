-- One ECS-owned panel can inspect any live entity. The fixed controls edit its
-- PhysicsBody; additional controls come from engine.export declarations.
local ids = assert(ecs.get_resource("scene.entities"))
local selected = assert(ids.player)
local page = 1
local page_size = 4
local slots = {ids.inspector_export_1, ids.inspector_export_2, ids.inspector_export_3, ids.inspector_export_4}
local labels = {}
for name, id in pairs(ids) do labels[id] = name end

local function sorted_entities()
    local entities = ecs.entities()
    local rank = {}
    for _, id in ipairs(entities) do
        rank[id] = id == ids.player and 0 or (ecs.has(id, "UiElement") and 2 or 1)
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
local function refresh()
    if not ecs.is_alive(selected) then selected = ids.player; page = 1 end
    text(ids.inspector_entity, labels[selected] or selected)
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
    text(ids.inspector_export_page, #exports == 0 and "No Lua exports" or ("Exports " .. page .. " / " .. pages))
    available(ids.inspector_export_prev, page > 1)
    available(ids.inspector_export_next, page < pages)
    if not has_body then text(ids.inspector_state, "Attach physics to this entity")
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
    selected = entities[((current - 1 + direction) % #entities) + 1]
    page = 1
end
local fields = {[ids.inspector_enabled]="enabled", [ids.inspector_collision]="collision_enabled",
    [ids.inspector_gravity]="gravity_enabled", [ids.inspector_radius]="radius", [ids.inspector_gravity_strength]="gravity",
    [ids.inspector_offset]="collision_offset_y"}

ecs.on("ui.action", function(action)
    local id, kind = action.id, action.kind
    if id == ids.inspector_prev and kind.type == "clicked" then choose(-1)
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
end)
local elapsed = 0
refresh()
ecs.system("inspector.refresh", function(dt)
    elapsed = elapsed + dt
    if elapsed < 0.25 then return end
    elapsed = 0
    local panel = ecs.get(ids.entity_inspector, "UiElement")
    if panel and not panel.collapsible.collapsed then refresh() end
end)
