-- Script-neutral scene requests travel through the ordinary ECS event queue.
-- The host validates and carries them out after the current script pass.
engine = { scene = {} }
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
