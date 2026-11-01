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
Lua can use `ecs.get_many(entity, {"A", "B"})` to read several
components through one bridge call. The result maps names to values, omits
missing components, and preserves present JSON null values as `ecs.null`.
Run `node scripts/benchmark_lua_batch.mjs --batch` to compare batched and
individual reads in the browser-Lua fixture.

The ECS browser updates only rows that actually change. It caches its tree
using `ecs.structure_revision()`, which changes on entity, parent,
component-membership, and adapter changes. Component value edits do not
invalidate it; adapters whose membership depends on values need their own
invalidation strategy.

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
browser Lua value bridge. It warms up Wasmoon and reports the median of nine
samples for 3,000 nested component reads and 3,000 pairs of scalar reads. The
fixture includes Lua, JavaScript, and JSON conversion, but substitutes a
JavaScript ECS response for Rust: use the in-browser profiler as well when
measuring end-to-end behavior.

## ECS query benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example query_bench` to compare named component queries
against sparse, dense, missing, and native component sets at two world sizes.
The example warms up each query and reports the median of seven samples.

## Rust scripting transport benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example script_bridge_bench` to measure JSON bridge reads,
liveness and presence checks, entity queries, resource fields, and empty event
drains. The benchmark uses both typed components and a larger dynamic component.
It does not include Lua, JavaScript, or rendering costs.

## Event dispatch benchmark

Run `node scripts/benchmark_event_dispatch.mjs` to compare repeated empty
channel checks with ordered batched checks. The fixture measures host-side JSON
transport only; use the browser profiler to measure the actual Rust/WASM path
and event handlers.

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example event_bench` to compare event burst and drain costs.

## UI layout benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ui-ecs --example layout_bench` to compare CPU layout extraction
for dense and sparse entity slots. The benchmark excludes GPU rendering and Lua.

## Browser Lua execution budget

Fixed-tick systems and event dispatch use an approximate ten-million-instruction
budget, checked every 1,000 Lua instructions. It resets for each host call and
does not count time spent waiting inside JavaScript or Rust callbacks. A runaway
script raises an error even through protected calls. Scene initialization keeps
its separate per-script wall-clock deadline.

Run `node scripts/benchmark_lua_tick.mjs` to compare the overhead of the
browser Lua tick and its ECS bridge using a small fixture.

## Frame-history benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ui --example frame_history_bench` to compare fresh and reused
buffers for a full minute of frame-time history.


## Streaming selection benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-render --example stream_selection_bench` to measure CPU-side LOD
selection for the checked-in 384-leaf level metadata and synthetic 1,024- and
10,000-leaf grids, in both radial and forward-ellipse modes. Each case moves and
turns the camera, warms the selector, and reports the median of five samples.
It excludes network, decoding, uploads, rendering, and GPU time. Compare builds
under the same conditions; background load and CPU clock changes affect results.


## Field-update benchmarks

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-scene-ecs --example field_write_bench` to compare full and
field-only JSON writes through the real physics and camera adapters. Run
`node scripts/benchmark_lua_field.mjs` to compare the Lua/JavaScript side
with an in-memory transport. Both are CPU microbenchmarks; they do not measure
rendering, collision processing, or frame rate.


## Camera lens decoding benchmark

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-scene --example lens_decode_bench` to compare decoding the
transient camera lens from cloned and borrowed JSON values. The benchmark
isolates CPU decoding; it does not measure rendering.


## Change-aware component reads

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-scene-ecs --example lens_revision_bench` for Rust lens read
costs, and `node scripts/benchmark_lua_lens_cache.mjs` for Lua/JavaScript
transfer costs. The latter uses an in-memory host and excludes rendering.
`cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example mutation_bench` compares ordinary and tracked
component mutations.

Native tracking is opt-in for component types whose changes pass through the
ECS world. Dynamic component stores track mutations automatically. Untracked
or externally mutable adapters return fresh data on every change-aware read;
a successful mutable borrow conservatively invalidates a tracked native
component even if its value is not changed.

Run `node scripts/benchmark_lua_gameplay_cache.mjs` to compare a batched
camera/controls/rig read with a fresh camera and cached read-only settings.
The fixture measures Lua/JavaScript conversion and uses an in-memory host;
it does not measure Rust execution, rendering, or overall frame rate.

Run `cargo run --locked --release --target-dir .artifacts/target -p
countersplat-ecs --example dynamic_revision_bench` for dynamic reads and
writes, and `node scripts/benchmark_lua_weapon_cache.mjs` for the
first-person Lua/JavaScript transfer fixture. Neither measures GPU frame rate.

Run `node scripts/benchmark_output_drain.mjs` to compare individual
output-channel drains with ordered nonempty-channel consumption. The fixture
measures browser-host JSON and dispatch overhead without Rust or rendering.

Run `node scripts/benchmark_lua_voxel_cache.mjs` to compare repeated
voxel settings and status transfers with unchanged cached snapshots. This
fixture measures Lua/JavaScript overhead, without Rust, collision, or rendering.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render benchmark_depth_uniform_packing -- --ignored --nocapture`
to compare depth-sort uniform packing. This CPU microbenchmark excludes GPU work.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_upload_preparation -- --ignored --nocapture`
to compare pending-upload ordering and loaded-LOD lookup. This measures CPU
preparation only, without network, encoding, GPU uploads, or rendering.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_fade_preparation -- --ignored --nocapture`
to compare CPU preparation of active streaming crossfades.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_retired_centre_preparation -- --ignored
--nocapture` to compare CPU preparation of retired splat slots. GPU transfer
and rendering time are excluded.

Run `node scripts/benchmark_lua_physics_skip.mjs` to compare the player
body's duplicate read with the per-body skip predicate. This fixture measures
Lua/JavaScript transport and excludes Rust physics and rendering.

Run `$env:LOD_UPLOAD_LEVEL = (Resolve-Path 'assets/levels/<level-id>').Path`
and `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_reused_upload_buffers -- --ignored
--nocapture --test-threads=1` to compare fresh and reused CPU staging buffers
on a local, separately acquired LOD level. The benchmark compares output bytes
and excludes decoding, GPU uploads, and rendering. Reuse is limited to typical
leaf sizes; unusually large uploads use temporary buffers.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_lod_label_staging -- --ignored --nocapture
--test-threads=1` to compare fresh and reused LOD-label preparation. The
CPU-only benchmark excludes the GPU transfer and rendering.

Run `node scripts/benchmark_lua_changed_reads.mjs` to compare the original
wrapper-table response, a table request with a scalar response, and scalar
arguments with a scalar response on the browser Lua bridge. This fixture measures Lua/JavaScript transport; it does not include
Rust execution, GPU rendering, or frame rate.

Run `node scripts/benchmark_lua_scalar_reads.mjs` to compare Lua request
and response tables with scalar arguments and direct results for a camera read
and a presence check. The in-memory fixture excludes Rust and GPU work.

Run `node scripts/benchmark_lua_query.mjs` to compare the general Lua query
encoder with the one-name query path. The in-memory fixture measures
Lua/JavaScript transport and excludes Rust query execution and GPU work.

Run `node scripts/benchmark_lua_field_writes.mjs` to compare the general
Lua field encoder with validated scalar, three-number vector, and atomic
two-field writes. The fixture excludes Rust validation, collision, and rendering.

Run `node scripts/benchmark_lua_get_many.mjs` to compare two-component
batch reads through the general and scalar-request browser bridges. The
in-memory fixture excludes Rust component reads and rendering.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-ecs benchmark_component_token_matching -- --ignored --nocapture`
to compare revision-token matching in isolation. The dynamic revision
benchmark above measures the surrounding ECS read path.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_direct_depth_keys -- --ignored --nocapture
--test-threads=1` to compare GPU timestamp measurements of the depth-key
shader with and without visibility counting. It requires timestamp-query
support and measures only that shader, not the rest of sorting or rendering.

Run `cargo test --locked --release --target-dir .artifacts/target -p
countersplat-render --lib benchmark_uniform_float_bytes -- --ignored
--nocapture --test-threads=1` to compare allocating and borrowing matrix
uniform bytes. This isolates CPU encoding and excludes GPU work.
