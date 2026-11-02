-- Escape opens the pause scene, which owns navigation to the level selector.
ecs.on("input.key", function(key)
    if key.key == "Escape" and key.pressed and not key.is_repeat and not ecs.get_resource("input.binding_capture") then
        engine.scene.push("pause_menu")
    end
end)
