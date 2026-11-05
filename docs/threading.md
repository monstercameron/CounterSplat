# Threading design

## Decision

On native targets, use long-lived, named owner threads with bounded message queues. Give each active subsystem its
own owner: simulation/ECS, scripting, physics, asset loading, and rendering. The host's platform event loop remains
on the thread required by its windowing system. A subsystem constructs and owns its mutable state on its worker;
other threads send data-only commands and receive replies, snapshots, or events. An idle or unused subsystem need
not be started merely to represent a crate.

The first implementation uses Rust's standard threads and bounded channels. This is a small dependency surface and
makes ownership and backpressure explicit. A full game host must wire each real subsystem to its corresponding
worker; a worker supervisor alone does not make a single-threaded application multithreaded.

## Ownership and message flow

- **Simulation/ECS:** owns the authoritative world. It applies game commands, advances the fixed-step clock, and
  publishes immutable, versioned data for other subsystems. No renderer or Lua object lives in the ECS crate.
- **Lua:** constructs and owns its VM on the scripting thread. Script-visible operations cross a data-only bridge to
  the ECS owner. If using synchronous request/reply, the ECS worker must continue serving requests and must never
  synchronously wait for the same script to finish; otherwise the two workers can deadlock. A tick-batched snapshot
  and command-buffer interface is the preferred direction for deterministic frame scheduling.
- **Physics:** owns its Rapier world. It accepts body/step commands and returns poses and collision events for the
  simulation owner to apply at a defined tick boundary.
- **Assets:** performs file/network I/O and CPU decoding. It sends owned decoded data or explicit failures to the
  consumer; bounded queues prevent unbounded memory growth when the renderer is slower than loading.
- **Rendering:** owns the device/queue, scene pool, GPU submission, and presentation resources. It consumes the latest
  immutable camera/scene snapshot and resize requests. Avoid waiting for physics or Lua while holding GPU or world
  locks. Drop obsolete visual snapshots rather than allowing an unbounded frame backlog.
- **Platform host:** owns window creation and event dispatch where the OS requires it. A future native window host
  can wake its event loop through a proxy and pass suitable render work to the render owner.

Use sequence numbers or tick IDs on snapshots and results so consumers can identify stale work. Decide queue-full
behavior for each message type: commands that must be applied need a visible error or retry; replaceable visual
snapshots may be coalesced. Shut down producers before consumers, then drain accepted work and join all workers.

## Implemented integration

`countersplat-runtime` provides native workers and bounded command queues, but does not automatically move any
subsystem. The native viewer creates its asset worker and renderer worker separately: SOG decoding and level metadata
I/O happen on the asset owner, while the renderer constructs and owns its device, pipelines, streaming pool, and GPU
operations. The renderer requests decoded chunks from the independent asset worker and finishes before that worker
shuts down. The CLI waits for this one-shot job; it does not represent an interactive event loop.

`countersplat-threaded-demo` creates actual ECS and Rapier worlds on separate runtime workers and a Lua VM on a third
thread. Lua sends ECS commands through the ECS worker; the host advances physics and publishes the resulting pose
back to ECS, where a Lua system reads it. Queues are bounded and Lua is stopped before the ECS owner. The two examples
establish five real subsystem owners, but do not yet compose all five into one continuously running host. The
real-time demo now drives ECS, Lua, and physics with fixed ticks; the viewer separately uses the same scheduler for
live offscreen frame requests. A unified multithreaded host, snapshot versioning, collision-event delivery, and browser Lua/physics integration
remain work. The standalone native window currently runs rendering on its window thread. See [timing](timing.md) for the clock and pacing contracts.

## Browser host

`countersplat-web` renders directly to a WebGPU canvas and advances the fixed-step scheduler from
`requestAnimationFrame`. A short-lived metadata worker fetches and parses level chunk descriptors and builds the
asset inventory; the original descriptor bytes transfer to the page owner for renderer setup. The worker reports
coalesced progress and is terminated on completion, failure, or cancellation. Metadata fetch concurrency is bounded,
and the output preserves file order. Single-scan metadata uses the same path without a network round trip.

A pool of one to eight dedicated workers fetches and decodes SOG textures based on logical CPU count (eight from 17
logical cores). Each decode is a single-threaded WebP/wasm job; on a 20-thread machine 24 Nelson chunks took 1755 ms
on one worker, 630 ms on four, 473 ms on eight and 423 ms on twelve, which is why the cap moved from four to eight.
Streamed chunks are selected by the renderer with no hidden worker queue, so available workers receive current
priorities. When a level marks splats as preloaded, the same bounded pool decodes multiple chunks concurrently while
preserving the original chunk order. A level with fewer chunks than workers (a single-file scan is one chunk) would
leave workers idle, so its chunk is split into texture groups (means_l, means_u, quats, scales, sh0, SH streams) that
different workers decode and the page merges again: the 2.5 M-splat Barn preload went from about 315 ms to 168 ms
with identical bytes. Finished chunks are queued and copied into the renderer from the frame loop within a 3 ms
budget (at least one per frame), so a burst of completed workers cannot stall a single frame; chunks waiting in that
queue count against idle workers, which bounds memory. Streaming 20 Nelson chunks inserted over 255 ms with the
worst frame 8.4 ms. Preload failure or cancellation terminates the pool. Decoded data is transferred to the canvas
owner for validated GPU upload; retained preload bytes have a 512 MiB limit, with up to one additional result in
flight per worker. A separate worker decodes Radiance HDR panoramas, and a voxel worker owns collision queries and
debug geometry. The browser allows one submitted GPU frame in flight. Frame-time measurements describe CPU submission
intervals, not GPU execution or display presentation timestamps.

### Rust host code, JavaScript glue

Host logic is Rust; JavaScript is kept to glue (DOM events, worker bootstraps, calls into the module). In Rust now:

- Chunk decoding and streaming (`apps/web/src/chunks.rs`): the worker side (`chunk_worker_handle`: fetch, decode,
  transfer), the worker pool, preloading with stream-split jobs (`preload_level_chunks`), and the per-frame insert
  budget, retries and requests in `Viewer::pump_chunks`. Planning rules and their tests: `countersplat_core::chunk_jobs`.
  `preview/decoder.js` is a three-line bootstrap.
- Level metadata loading (`apps/web/src/metadata.rs`, `loadLevelMetadata` / `metadata_worker_handle`) and the asset
  policies and level inventory (`countersplat_scene::asset_policy`); `metadata-worker.js` is a bootstrap.
- Display and HUD preferences, render resolution, presentation mapping and the viewport resources scene scripts read
  (`countersplat_scene::display`, `apps/web/src/display.rs`, `Viewer::apply_display`, `Viewer::menu_settings`).
- Profiling counters (`countersplat_core::profile`), gameplay pointer buttons and pointer lock
  (`countersplat_ui::pointer`), world-line layers (`countersplat_scene::line_layers`), UI layout, control, rig and other
  stored settings (`countersplat_scene::host_preferences`, `apps/web/src/prefs.rs`), impact/projectile/hitbox packing,
  and first-person footage scheduling (`countersplat_scene::first_person_frames`, `apps/web/src/footage.rs`).
- Level collision (`apps/web/src/voxels.rs`): the voxel worker (`voxel_worker_handle`) and the per-entity movement,
  ground, clearance and trace coordination, owned by the Viewer (`start_voxels`, `voxel_receive`, `voxel_tick`, ...).
  The rules are `countersplat_scene::voxel_host`, written against a small `Host` trait so they are unit-tested without
  a browser. Because `drain` and `emit` are ECS operations on the Viewer, the coordinator calls its own world directly
  instead of round-tripping through JavaScript. Worker replies are queued and applied at the start of each frame
  (`voxel_receive`), before input and scripts read the world.
- The level map (`apps/web/src/minimap.rs`, rules in `countersplat_scene::minimap`): request validation and
  coalescing, the map worker with its own voxel copy or, for a level without voxels, splat centres decoded from the
  finest LOD within 3 M splats, and the round map raster. Results reach the scene after the page state is released, so
  a scene may request again from its result handler.
- Web Audio playback (`apps/web/src/audio.rs`, `SceneAudio`; mixing rules in `countersplat_scene::audio_mix`).
- The opt-in CPU stack capture (`apps/web/src/stacks.rs`, `FrameStackCapture`; capture, summary and GPU-frame
  matching in `countersplat_core::frame_stacks`). It binds the browser's JS Self-Profiling `Profiler` directly.

Each moved module's JavaScript tests were ported to Rust unit tests. The workers (`decoder.js`, `metadata-worker.js`,
`voxel-worker.js`, `minimap-worker.js`, `skybox-worker.js`) are bootstraps that load the module and forward messages.
Still JavaScript: the page orchestration (`main.js`: DOM events, level launch sequencing and the frame loop calling
into Rust) and the Lua bridge around the third-party wasmoon VM (`lua_runtime.js`). The native build runs Lua with
mlua, which does not target `wasm32-unknown-unknown`, so moving the bridge needs a Lua VM that builds for that target.

The page thread still owns the browser ECS, Lua VMs, WebGPU orchestration, and the renderer's initial Rust stream
construction. Physics geometry queries and asset preparation run on workers, but this is not a fully separated browser
subsystem architecture. Moving Lua and ECS to workers needs a tick-based snapshot and command protocol rather than
synchronous calls across threads. Rust's ordinary `wasm32-unknown-unknown` target does not provide functioning
`std::thread::spawn`; additional browser owners need a Web Worker backend. Shared WebAssembly memory additionally
needs cross-origin isolation and compatible browser configuration. WebGPU availability must be detected, and the
current host reports an error when unavailable.

## Evidence

- [Rust `std::sync::mpsc`](https://doc.rust-lang.org/std/sync/mpsc/) documents bounded `sync_channel` queues.
- [Rust `wasm32-unknown-unknown` platform support](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html)
  documents that ordinary thread creation does not work on that target.
- [winit `EventLoop`](https://docs.rs/winit/latest/winit/event_loop/struct.EventLoop.html) documents main-thread
  portability and `EventLoopProxy`.
- [mlua](https://docs.rs/mlua/latest/mlua/) documents the VM's default non-`Send` behavior and optional feature.
- [wgpu `Device`](https://docs.rs/wgpu/30.0.1/wgpu/struct.Device.html) documents native device traits; host/platform
  object ownership still needs to follow the platform lifecycle.
- [MDN cross-origin embedder policy](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Embedder-Policy)
  explains the isolation requirement for shared-memory capabilities.
- [MDN worker WebGPU](https://developer.mozilla.org/en-US/docs/Web/API/WorkerNavigator/gpu) and
  [OffscreenCanvas contexts](https://developer.mozilla.org/en-US/docs/Web/API/OffscreenCanvas/getContext) describe
  browser worker APIs and compatibility.
