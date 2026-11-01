-- Navigation and player controls both receive copies of the same ECS events.
local ids = assert(ecs.get_resource("scene.entities"), "gameplay entities are missing")
ecs.on("input.key", function(key)
    if key.key == "Escape" and key.pressed and not key.is_repeat and not ecs.get_resource("input.binding_capture") then
        engine.scene.push("pause_menu")
    end
end)
ecs.on("ui.action", function(action)
    if action.id == ids.back_to_levels and action.kind.type == "clicked" then
        engine.scene.replace("level_selector")
    end
end)
