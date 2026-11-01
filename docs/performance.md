# Browser frame performance

The browser preview uses fixed simulation ticks and presents on the browser's
animation cadence. Rendering may submit up to two frames at once. The bounded
queue avoids stalling for every completion callback, while preventing an
unbounded backlog. Surface and render-target resizes wait for all outstanding
frames to finish. Queue writes remain ordered with their submission; see the
[wgpu queue documentation](https://docs.rs/wgpu/latest/wgpu/struct.Queue.html).

Lua still owns gameplay decisions. The physics step reuses data it already read,
and `ecs.get_resource_field(name, field)` reads individual resource fields
without copying whole maps while still observing runtime changes. The level and
voxel panels fetch their controls only when their source settings
change, and settled camera/posture systems avoid writing unchanged components.
The ECS browser updates only rows that actually change.

## Profiling

Open the local preview with `?profile=1` and launch a level. The existing
`#metrics` element exposes a read-only `data-profile` JSON snapshot. In the
browser developer console, inspect it with:

```js
JSON.parse(document.querySelector('#metrics').dataset.profile)
```

The snapshot contains the last 600 animation callback and submitted-frame
intervals, along with CPU time spent in rendering, Lua systems, event dispatch,
output, streaming, and the full callback. Each series reports its mean,
median, p95, p99 and maximum in milliseconds. Lua system and ECS operation
counters are cumulative for the current scene; GPU busy skips are also
cumulative and can include loading. Reload or start another scene for a fresh
measurement.

These are CPU wall times and submission intervals, not GPU timestamps or actual
monitor-present feedback. The optional per-system profiler adds overhead.
Compare the same level, view, resolution, viewport, settings and measurement
duration, and let loading and shader compilation settle before comparing.
`?profile` is optional; normal play does not collect these detailed samples.

## Browser Lua bridge benchmark

Run `node scripts/benchmark_lua_bridge.mjs` to compare changes to the
browser Lua value bridge. It warms up Wasmoon and reports nine sorted timings
for 3,000 reads of a nested component. The fixture includes Lua, JavaScript,
and JSON conversion, but substitutes a JavaScript ECS response for Rust: use
the in-browser profiler as well when measuring end-to-end behavior.

## ECS query benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example query_bench` to compare named component queries
against sparse, dense, missing, and native component sets at two world sizes.
The example warms up each query and reports the median of seven samples.

## Rust scripting transport benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example script_bridge_bench` to measure JSON bridge reads,
presence checks, resource fields, and empty event drains. The benchmark uses
both typed components and a larger dynamic component. It does not include Lua,
JavaScript, or rendering costs.

## Event dispatch benchmark

Run `node scripts/benchmark_event_dispatch.mjs` to compare repeated empty
channel checks with ordered batched checks. The fixture measures host-side JSON
transport only; use the browser profiler to measure the actual Rust/WASM path
and event handlers.

## UI layout benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ui-ecs --example layout_bench` to compare CPU layout extraction
for dense and sparse entity slots. The benchmark excludes GPU rendering and Lua.
