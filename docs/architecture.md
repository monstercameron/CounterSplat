# Architecture

## Dependency direction

```text
application / host
  ├── audio (Kira / CPAL / Symphonia)
  ├── audio-ecs ──────► audio, ecs
  ├── audio-runtime ──► audio, runtime
  ├── render ─────────► assets
  │      └────────────► shaders
  ├── physics ────────► assets
  ├── scripting-lua ──► ecs
  ├── scene-ecs ──────► scene, ecs
  ├── ui-ecs ─────────► ui, ecs
  ├── ui-wgpu ────────► ui
  ├── web host ───────► core, assets, render, ecs, scene and UI adapters
  ├── runtime
  └── core
```

`audio`, `core`, `ecs`, `scene`, `ui`, `runtime`, `assets`, and `shaders` do not depend on another CounterSplat crate. `render` does not depend on physics;
`physics` does not depend on render. The application owns orchestration, device creation, input, file or network
I/O, gameplay, and presentation. `scripts/check_boundaries.py` checks the dependency policy and literal source
includes. It is intentionally small enough to run without a Rust build.

## Runtime responsibilities

- **Audio:** owns layered Kira mixing and device playback; a separate ECS adapter exposes scene components and snapshots. The host owns the thread, asset loading and command routing. See [audio](audio.md).
- **Core:** `time::FixedClock` turns elapsed host time into bounded simulation ticks and an interpolation fraction.
  `rate` preserves fractional frequencies, `scheduler` combines ticks and frame deadlines, `pacing` provides fixed
  and measured-load adaptive decisions, and `log` provides a bounded structured log. See [timing](timing.md).
- **ECS:** owns entities, typed and dynamic components, resources, hierarchy, queries, events, and schedules. Its
  serializable commands and snapshots expose neutral data without depending on any scripting or rendering backend.
- **Lua:** owns the Lua 5.4 VM and maps script operations to ECS commands through a replaceable bridge. Scripts never
  borrow the world; Rust components need explicit adapters before they become script-visible.
- **Scene:** `scene` defines versioned scene manifests, resource catalogs, and renderer-independent `Skybox` settings.
  `scene-ecs` instantiates manifest layers, validates writes, and extracts the active sky without depending on a
  renderer or scripting VM. See [scene JSON](scenes.md) and [skyboxes](skybox.md).
- **UI:** data-only elements and frame measurements live in `ui`. `ui-ecs` extracts ECS snapshots and applies
  validated events; `ui-wgpu` composites them using egui. These adapters do not depend on each other. See [UI](ui.md).
- **Runtime:** supplies native subsystem workers, bounded queues, reply tickets, and a latest-value mailbox. The host
  decides which real systems to start, their message contracts, tick order, and shutdown order.
- **Assets:** `sog` decodes SOG v2 bundles and chunks; `lod` parses chunk ranges and level trees; `svo` reads sparse
  collision voxels. `codebook` is the shared, validated codebook API used by both asset merging and streaming.
  `viewer` parses optional camera hints; `environment` decodes bounded Radiance HDR panoramas. Byte-based entry points
  let browser hosts supply fetched data.
- **Shaders:** `preprocess`, `binding`, `declarations`, and `pipeline` turn the preserved upstream WGSL chunks into
  complete shader stages and a binding table. Radix kernels are exposed through the shader crate's public API.
- **Render:** `scene` uploads an asset or a streaming pool; `sort` performs GPU depth sorting and visibility-key
  generation; `stream` selects LODs, requests chunks, allocates pool ranges, and crossfades them. A host supplies
  the wgpu device, queue, camera, render targets, and time. `env` projects a panorama to cubemap faces and renders a rotating HDR sky background.
- **Physics:** `world::World` owns Rapier, validates its public inputs, scopes body handles to their world, and builds
  static floors or obstacle boxes from SVO geometry. Floor and obstacle generation is independent of rendering.
- **Viewer:** demonstrates asset I/O and offscreen rendering on separate workers, including a finite clock-driven
  preview with bounded in-flight work. **Web host:** presents to a browser WebGPU canvas and uses a Web Worker for
  SOG decoding. **Threaded demo:** runs ECS, Lua, and physics on their own workers, including a real-time fixed-step
  mode. These are example hosts, not dependencies of engine libraries.

## Streaming sequence

1. Parse `lod-meta.json` and collect each chunk's metadata. Construct `LevelStream` with a pool budget.
2. Create a scene pool using `shared_meta` and `palette_texture_rows`, then call `init_pool`.
3. Each frame, select LODs using the focus, view direction, optional clip matrix, and a monotonic time.
4. Fetch `requests()` in the host; return decoded bundles with `insert` or report failures with `failed`.
5. Call `apply` to update the GPU pool, then prepare and draw the scene. Repeat while chunks or crossfades are pending.

The viewer's `render-level` command runs selection and GPU operations on the rendering worker and requests decoded
chunks from a separate asset worker. The command-line host waits for completion; a real-time or browser host should
poll work without blocking its event loop.

## Rendering and coordinates

Matrices in `render::camera` are column-major. The renderer accepts a scene model matrix, a view matrix, and a
projection; the host is responsible for making these agree with the scan's coordinate convention and physics world.
The viewer's optional `z180` argument demonstrates one source-scan convention without hard-coding it into the engine.

Depth sorting and LOD selection remain on the GPU/CPU respectively. Scene drawing uses the active pool prefix and
lets the vertex stage reject invisible splats. This portable path can do more vertex work than the original
GPU-generated indirect draw; it avoids a blank-frame problem observed with indirect validation on the development
machine. Profile and validate other backends before introducing an optimized indirect path.

## Extension rules

Add game-specific actors, input, UI, and effects in application or feature crates that depend on these libraries.
Pass data through public types instead of reaching into another crate's files. Keep format decoding independent of
GPU and physics backends. Add a narrow adapter at the application boundary when two systems need to collaborate.
Test cross-system behavior at that boundary, including a browser build when changing portable APIs.
