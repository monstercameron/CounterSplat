# Asset loading and level transitions

The browser level selector shows a WebGPU canvas loading panel when a level is launched. The host loads
required resources, builds the renderer and Lua scenes, and enters gameplay only after those steps succeed.
The Cancel button aborts downloads and decoding while the preload is in progress and returns to the selector.
Failed preloads return to the selector with an error. Loading another level starts a fresh transition.

The selector's `loader_overlay` entity owns an `AssetLoader` component. Its script-facing fields are `status`
(`idle`, `loading`, `complete`, `failed`, or `cancelled`), `phase`, normalized `progress`, and the current
phase's `completed` and `total` task counts. The host updates the component and the UI widgets; progress is a
weighted estimate of phases, not a byte percentage. Register `countersplat_scene_ecs` adapters when instantiating
this UI layer. The browser selector exposes its world through the standard ECS operation transport.

## Scene resources

Scene assets and scripts in `assets/scenes/*.json` declare their loading policy:

```json
{
  "assets": {
    "sky": { "kind": "radiance_hdr", "uri": "../images/environment.hdr", "load": "preload" },
    "level": { "kind": "sog_level", "uri": "/level/", "load": "streamable" }
  },
  "scripts": {
    "controls": { "language": "lua", "uri": "../scripts/controls.lua", "autoload": true, "load": "preload" },
    "optional": { "language": "lua", "uri": "../scripts/optional.lua", "autoload": false, "load": "streamable" }
  }
}
```

`load` defaults to `preload` for older scene files. Preloaded scene assets and scripts are fetched before gameplay;
preloaded HDR skies are also decoded and installed. Preloaded source bytes are retained for the gameplay session.
The browser executes autoload scripts and scripts listed in `resources.preview.browser_scripts`; both must be marked
`preload`. The `browser_scripts` list determines execution order; additional autoload scripts follow in catalog
order. A streamable optional script is not executed automatically. The current host starts streamable HDR skies
and splat chunks asynchronously; other asset kinds need their subsystem to implement on-demand loading and decoding.

## Level resources

Each entry in `assets/levels/catalog.json` has an `assets` policy, for example:

```json
"assets": {
  "manifest": "preload",
  "viewer_settings": "preload",
  "chunk_metadata": "preload",
  "splat_chunks": "streamable"
}
```

The level manifest, optional viewer-settings file, and chunk metadata initialize the renderer and therefore must be
`preload` in this browser host. A missing optional viewer-settings file uses the renderer's camera fallback. Set
`splat_chunks` to `streamable` to transition once prerequisites are ready and fetch visible chunks as needed. Set it
to `preload` to fetch and decode every chunk before the transition and retain the decoded chunks for the level's
lifetime. The GPU still uploads visible splats incrementally according to its frame budget. The per-level policy
controls the gameplay scene's `sog_level` asset. Absent level policies default to these same settings.

Scene-resource bytes and decoded preloaded chunks each have a 512 MiB browser limit; larger scans should use
`streamable`. These are host limits, not restrictions imposed by the scene format. Other engine hosts can implement
the same policy vocabulary with their own asset stores and thread ownership.

## Browser concurrency

The gameplay scene's `resources.preview` can tune asset work without changing engine code:

```json
"preview": {
  "chunk_decoder_workers": "auto",
  "metadata_fetch_concurrency": 12
}
```

`chunk_decoder_workers` selects `"auto"` (one to eight workers based on the browser's reported logical cores: 1, 2,
3, 4, 6 and 8 at up to 2, 4, 8, 12, 16 and more cores), or an explicit integer from 1 to 8. It applies to both
streamed and preloaded splats. Decoded chunks are copied into the renderer from the frame loop, within a 3 ms budget
per frame (at least one chunk), so a burst of finished workers cannot stall one frame. Metadata fetch concurrency
can be 1 to 32; fetch and parsing happen on a dedicated temporary worker. More workers can improve throughput,
but also use more memory and CPU; profile on the intended device. These settings do not move the browser's Lua,
ECS, or GPU submission off the page thread. See [thread ownership](threading.md).
