-- Declare scene-specific component tuning through the same Lua API available to games.
-- The generic entity inspector discovers these fields without owning the components.
local ids = assert(ecs.get_resource("scene.entities"))
local level = ids.active_level
if level and ecs.has(level, "LevelSettings") then
    engine.export(level, "draw_distance", {component="LevelSettings", field="draw_distance",
        label="Draw distance", min=10, max=1000})
end
if level and ecs.has(level, "VoxelCollider") then
    engine.export(level, "voxel_enabled", {component="VoxelCollider", field="enabled", label="Enable voxels"})
    engine.export(level, "voxel_visible", {component="VoxelCollider", field="visible", label="Show voxels"})
    engine.export(level, "voxel_physics", {component="VoxelCollider", field="physics", label="Level collision"})
end
local skybox = ids.skybox
if skybox and ecs.has(skybox, "Skybox") then
    local sky = ecs.get(skybox, "Skybox")
    engine.export(skybox, "sky_enabled", {component="Skybox", field="enabled", label="Show sky"})
    engine.export(skybox, "sky_exposure", {component="Skybox", field="exposure", label="Exposure", min=0, max=64})
    engine.export(skybox, "sky_rotation", {component="Skybox", field="rotation_degrees", label="Rotation",
        min=math.min(-360, sky.rotation_degrees), max=math.max(360, sky.rotation_degrees)})
end
