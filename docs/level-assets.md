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
The scene's `resources.preview.capacity` (three million splats) is the pool's floor; a streamed level's pool is sized to about 70% of its finest-detail splat count, up to `resources.preview.max_capacity` (six million), which bounds GPU memory: a pool costs about 49 bytes a splat (measured from the GPU allocator; 76 before the sorter reused its scratch buffers and a level without spherical harmonics stopped allocating per-slot SH labels), so six million slots is about 280 MiB. Part of the pool is reserved for transitions. In a dense scan this matters: the 17-million-splat Nelson Ghost Town has 8.8 million finest-detail splats in one 50 m block, and growing the pool from 3 to 6 million raised image fidelity by about 2 dB against a full-detail reference and cut LOD swaps by a third. Fragmented free
ranges are compacted incrementally so a centered fine-detail region is not blocked by scattered holes. The footer
shows how many regions currently have LOD 0 resident. A manual LOD request overrides the automatic preference.

The browser fetches and decodes chunks on a bounded pool of one to four workers, depending on available CPU
concurrency. It asks the streamer for only as many chunks as workers can start immediately, so a camera movement
can change the next request's priority. GPU uploads remain budgeted and run only when a frame can be submitted. Empty regions use already-loaded
coarser placeholders when available, without filling the pool with unrequested fine ranges.

Each visible LOD replacement finishes its fade before the same region changes again. If a region leaves while it
is still appearing, it fades out from its current opacity. When the pool cannot fit both the old and new visible
ranges, smooth mode retains the old range until a fade or compaction makes space; detail can remain coarser if
the configured pool is too small to hold both. This avoids an abrupt visible swap.

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
