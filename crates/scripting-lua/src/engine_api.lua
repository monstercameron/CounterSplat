-- Script-neutral scene requests travel through the ordinary ECS event queue.
-- The host validates and carries them out after the current script pass.
engine = { scene = {}, physics = {} }
local function scene_id(id)
    assert(type(id) == "string" and #id > 0 and #id <= 256, "scene ID must be a nonempty string")
    return id
end
function engine.scene.push(id)
    ecs.emit("engine.scene", { action = "push", scene = scene_id(id) })
end
function engine.scene.pop()
    ecs.emit("engine.scene", { action = "pop" })
end
function engine.scene.replace(id)
    ecs.emit("engine.scene", { action = "replace", scene = scene_id(id) })
end

-- A script can expose an existing ECS field to an inspector without introducing
-- another copy of the value. Exports are per entity and remain ordinary ECS data.
function engine.export(entity, name, spec)
    assert(type(entity) == "string" and ecs.is_alive(entity), "export needs a live entity")
    assert(type(name) == "string" and #name > 0 and #name <= 64, "invalid export name")
    assert(type(spec) == "table" and type(spec.field) == "string" and
        ((type(spec.component) == "string") ~= (type(spec.resource) == "string")),
        "export needs a field and exactly one component or resource")
    local target
    if spec.component then target = ecs.get(entity, spec.component)
    else target = ecs.get_resource(spec.resource) end
    assert(type(target) == "table", "export target is missing")
    local value = target[spec.field]
    local kind = type(value)
    assert(kind == "boolean" or kind == "number", "exports support boolean and number fields")
    local label = spec.label or name
    assert(type(label) == "string" and #label > 0 and #label <= 64, "invalid export label")
    local entry = {name=name, component=spec.component, resource=spec.resource, field=spec.field, label=label, kind=kind}
    if kind == "number" then
        assert(type(spec.min) == "number" and type(spec.max) == "number" and
            spec.min == spec.min and spec.max == spec.max and math.abs(spec.min) <= 10000000 and
            math.abs(spec.max) <= 10000000 and spec.min < spec.max and value >= spec.min and value <= spec.max,
            "numeric exports need finite bounds containing the current value")
        entry.min, entry.max = spec.min, spec.max
    end
    local all = ecs.get_resource("engine.exports") or {}
    local entries = all[entity] or ecs.array()
    local found = false
    for i, previous in ipairs(entries) do
        if previous.name == name then entries[i] = entry; found = true; break end
    end
    if not found then entries[#entries + 1] = entry end
    all[entity] = entries
    ecs.set_resource("engine.exports", all)
end
function engine.exports(entity)
    local all = ecs.get_resource("engine.exports") or {}
    return all[entity] or ecs.array()
end
function engine.unexport(entity, name)
    local all = ecs.get_resource("engine.exports") or {}
    local entries = all[entity]
    if not entries then return false end
    for i, entry in ipairs(entries) do
        if entry.name == name then
            table.remove(entries, i)
            all[entity] = #entries > 0 and entries or nil
            ecs.set_resource("engine.exports", all)
            return true
        end
    end
    return false
end
function engine.set_export(entity, name, value)
    for _, entry in ipairs(engine.exports(entity)) do
        if entry.name == name then
            assert(type(value) == entry.kind, "export value has the wrong type")
            if entry.kind == "number" then
                assert(value == value and value >= entry.min and value <= entry.max, "export value is out of bounds")
            end
            local target
            if entry.component then target = ecs.get(entity, entry.component)
            else target = ecs.get_resource(entry.resource) end
            assert(type(target) == "table", "export target was removed")
            target[entry.field] = value
            if entry.component then ecs.set(entity, entry.component, target)
            else ecs.set_resource(entry.resource, target) end
            return
        end
    end
    error("unknown exported value")
end

local function position_component(entity)
    if ecs.has(entity, "PlayerCamera") then return "PlayerCamera" end
    if ecs.has(entity, "Transform") then return "Transform" end
end
local function finite_number(value)
    return type(value) == "number" and value == value and math.abs(value) <= 10000000
end

local motion_version = {}
function engine.physics.motion_version(entity) return motion_version[entity] or 0 end

-- Submit a ground query to the host. Results arrive through physics.ground_result;
-- scripts decide whether and how to use them.
function engine.physics.probe_ground(entity, position, radius, max_drop, token)
    assert(type(entity) == "string" and ecs.is_alive(entity), "ground probe needs a live entity")
    assert(type(position) == "table" and #position == 3, "ground probe needs a 3D position")
    assert(finite_number(radius) and radius >= 0 and radius <= 1, "invalid ground probe radius")
    assert(finite_number(max_drop) and max_drop >= 0.05 and max_drop <= 16, "invalid ground probe distance")
    assert(type(token) == "string" and #token > 0 and #token <= 64, "invalid ground probe token")
    local copy = ecs.array()
    for i = 1, 3 do assert(finite_number(position[i]), "invalid ground probe position"); copy[i] = position[i] end
    ecs.emit("physics.ground_request", {entity=entity, position=copy, radius=radius, max_drop=max_drop, token=token})
end

-- Move a camera or transform and send the movement to the collision owner when
-- one is ready. Hosts without a collision worker retain direct movement.
function engine.physics.move(entity, delta)
    assert(type(delta) == "table" and #delta == 3, "movement needs three components")
    local name = assert(position_component(entity), "entity needs PlayerCamera or Transform")
    local pose = assert(ecs.get(entity, name))
    local applied = ecs.array()
    for i = 1, 3 do
        assert(finite_number(delta[i]), "movement must be finite")
        local next_value = math.max(-10000000, math.min(10000000, pose.position[i] + delta[i]))
        applied[i] = next_value - pose.position[i]
        pose.position[i] = next_value
    end
    ecs.set(entity, name, pose)
    if applied[1] ~= 0 or applied[2] ~= 0 or applied[3] ~= 0 then
        motion_version[entity] = (motion_version[entity] or 0) + 1
    end
    local body = ecs.get(entity, "PhysicsBody")
    if body and body.enabled and body.collision_enabled and ecs.get_resource("voxel.capture_movement") then
        ecs.emit("physics.move", {entity=entity, component=name, delta=applied})
    end
end

-- Run this once per fixed simulation tick. Bodies with disabled physics keep
-- their stored velocity; collisions and gravity can be switched independently.
function engine.physics.step(dt, skip_entity)
    assert(finite_number(dt) and dt >= 0 and dt <= 0.25, "invalid physics tick")
    assert(skip_entity == nil or type(skip_entity) == "string", "invalid skipped entity")
    for _, entity in ipairs(ecs.query({"PhysicsBody"})) do
        local body = ecs.get(entity, "PhysicsBody")
        if entity ~= skip_entity and body and body.enabled and position_component(entity) then
            if body.gravity_enabled then
                body.velocity[2] = math.max(-200, body.velocity[2] - body.gravity * dt)
                ecs.set(entity, "PhysicsBody", body)
            end
            local delta = ecs.array()
            local moving = false
            for i = 1, 3 do delta[i] = body.velocity[i] * dt; moving = moving or delta[i] ~= 0 end
            if moving then engine.physics.move(entity, delta) end
        end
    end
end
