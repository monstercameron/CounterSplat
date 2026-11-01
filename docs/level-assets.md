# Level assets and LOD settings

The gameplay scene declares an `active_level` entity on its `level` layer, alongside a collapsible, draggable
**Level assets** canvas panel. The browser instantiates the level, player, and UI layers in the same ECS world,
so Lua and other gameplay systems can use the ordinary `scene.entities` and component APIs.

`LevelAssets` owns references and metadata, never file bytes or renderer objects. Its `source` names a
`sog_level` asset in the scene JSON. The host fills in the selected level ID, name, format, LOD count, and `files`
before starting gameplay scripts. File paths are relative to `assets/levels/<level_id>/`. Each record has a
`kind` (`manifest`, `viewer_settings`, `chunk_metadata`, `splat_data`, `collision`, or `other`) and a `load`
policy (`preload` or `streamable`). The host discovers manifest and texture references from actual metadata; when
available, the download inventory adds collision and other files. The browser separately loads a level's
`scene.voxel.json` and `scene.voxel.bin` into the voxel collision worker when they are present; inventory references
alone do not load arbitrary files. Duplicate and escaping paths are rejected. See [voxel collision](voxel-collision.md)
for the ECS settings and player collision.

`LevelSettings.lod` is `null` for automatic screen-space selection, or a zero-based LOD index. LOD 0 requests
the finest detail; higher numbers request coarser levels. In the panel, **Auto LOD** selects automatic behavior;
moving the slider selects a manual LOD, and checking Auto returns to automatic selection. A single-LOD scan shows
its asset count but disables these controls. Manual selection uses an available LOD at or coarser than the request when possible, or the coarsest available
level when a leaf has no coarser option. The GPU pool budget can also reduce detail. Chunks still load and transition incrementally.

The **Draw distance** slider controls the forward reach of the streaming region (10–1,000 world units).
The browser uses a forward-biased oval: its rear reach is 30% and its sideways reach is 55% of the selected
forward reach. An octree indexes level regions and their residency is tracked as the camera moves. A wider
retention boundary, a short delay, and complementary fades keep transitions from flickering at the edge.

In automatic mode, regions intersecting the central half of the camera view request LOD 0 first; nearby visible
regions also request it directly. This preference remains subject to available source data, asynchronous loading,
and the GPU pool: when all requested detail cannot fit, more distant or peripheral regions may use coarser LODs.
The browser's default pool holds three million splats and reserves part of it for transitions. Fragmented free
ranges are compacted incrementally so a centered fine-detail region is not blocked by scattered holes. The footer
shows how many regions currently have LOD 0 resident. A manual LOD request overrides the automatic preference.

Lua uses the same validated ECS adapters as other systems:

```lua
local ids = ecs.get_resource("scene.entities")
local assets = ecs.get(ids.active_level, "LevelAssets")
local settings = ecs.get(ids.active_level, "LevelSettings")
-- Request LOD 1 when the active scan has it.
if assets.lod_count > 1 then
    settings.lod = 1
    ecs.set(ids.active_level, "LevelSettings", settings)
end
-- Restore automatic LOD and set the forward draw distance.
settings.lod = ecs.null
settings.draw_distance = 150
ecs.set(ids.active_level, "LevelSettings", settings)
```

The scene JSON sets the initial components and UI arrangement; the selected level's file inventory is generated
at load time, so switching levels does not duplicate asset lists in gameplay scene JSON. The panel's Lua script
reflects subsequent component changes. Existing scene and level preload policies are described in
[asset loading](asset-loading.md).
