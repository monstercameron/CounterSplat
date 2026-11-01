-- Optional example: load this script on a Lua-capable host, then run "skybox.rotate".
-- Entity handles come from the layer instantiated from assets/scenes/gameplay.json.
local ids = ecs.get_resource("scene.entities")
local sky_entity = assert(ids and ids.skybox, "the scene layer needs a skybox entity")
ecs.system("skybox.rotate", function(dt)
    local sky = ecs.get(sky_entity, "Skybox")
    if sky then
        sky.rotation_degrees = (sky.rotation_degrees + 2 * dt) % 360
        ecs.set(sky_entity, "Skybox", sky)
    end
end)
