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

`countersplat-web` creates a WebGPU surface directly on the HTML canvas and advances the shared scheduler from
`requestAnimationFrame`. A dedicated Web Worker fetches and decodes SOG textures. A separate skybox worker decodes
Radiance HDR panoramas and converts them to cubemap faces. Both transfer decoded bytes to the canvas owner for
validated GPU uploads. The browser host allows one submitted GPU frame in flight and coalesces
later requests. The UI's frame-time measurements describe submission intervals, not GPU execution or display
presentation timestamps.

The browser host currently owns GPU and scheduling on the browser page thread and uses workers for asset decoding.
It does not yet compose the native ECS, Lua, and physics workers. Rust's ordinary `wasm32-unknown-unknown` target does
not provide functioning `std::thread::spawn`; additional browser subsystem threads require a Web Worker/message
backend. Shared WebAssembly memory additionally needs cross-origin isolation and compatible browser configuration.
WebGPU availability must be detected, and the current host reports an error when unavailable.

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
