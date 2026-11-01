-- This menu is an ordinary ECS UI scene. The host performs the requested transition.
local ids = assert(ecs.get_resource("scene.entities"), "menu UI entities are missing")
assert(ids.resume and ids.levels, "menu buttons are missing")
ecs.system("menu.navigation", function(_dt)
    local keys = ecs.drain("input.key")
    local actions = ecs.drain("ui.action")
    for _, key in ipairs(keys) do
        if key.key == "Escape" and key.pressed and not key.is_repeat then
            engine.scene.pop()
            return
        end
    end
    for _, action in ipairs(actions) do
        if action.kind.type == "clicked" then
            if action.id == ids.resume then
                engine.scene.pop()
                return
            elseif action.id == ids.levels then
                engine.scene.replace("level_selector")
                return
            end
        end
    end
end)
