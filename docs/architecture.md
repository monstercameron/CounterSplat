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

Depth sorting and LOD selection remain on the GPU/CPU respectively. The depth-key pass counts the splats in view and
a small compute pass turns that count into `draw_indexed_indirect` arguments, so scene drawing shades only that
prefix of the sorted order. In the benchmark below it cut a static view's frame time by about 30% on a desktop
NVIDIA GPU, because culled splats no longer pay four vertex-shader invocations each.

An earlier version drew the whole active pool prefix and let the vertex stage reject invisible splats, to avoid a
blank-frame problem seen with indirect draws on one development machine. That path remains as a runtime fallback:
`scene::set_indirect_draw(false)`, exposed to the browser preview as `?indirect_draw=0`. The indirect path has been
validated on one desktop GPU (Chrome WebGPU and native wgpu); validate other backends before relying on it, and
fall back if a frame comes up blank.

The depth sort is the engine's five-pass 4-bit radix sort. The shader crate also carries the engine's OneSweep
sort (`radix-sort/onesweep-*.wgsl`: three 8-bit passes with decoupled look-back), unused. It was wired in and
measured against the 4-bit sort on a desktop NVIDIA GPU with the 8.9 M-splat lobby scene: it ran 2–2.5 ms *slower*
per re-sort (about 9–10 ms against 7–8 ms per frame while the camera spins), passed the same order and stability
tests, and was reverted. It also needs subgroup support and can deadlock on GPUs without forward-progress
guarantees between workgroups, so any future attempt should be measured on each target vendor before it is enabled.

`cargo run --release -p countersplat-render --example splat_bench -- assets/levels/<id>` renders a level offscreen
and GPU-saturated, which keeps clocks high and gives stable medians; the browser preview is capped at the display's
refresh rate, so its GPU timings vary with clock state. Compare before and after on the same machine.

`cargo run --release -p countersplat-render --example stream_bench -- assets/levels/<id> seconds=90 loop=x,z,r
quality=16` evaluates the level loader on a streamed (multi-LOD) level. It drives `LevelStream` as the browser does
(same `StreamSettings`, four decode workers whose latency is each chunk's measured decode time, a fixed 60 Hz step)
along a scripted camera loop, and reports pop-in (LOD switches, frames with a leaf in view and nothing drawn,
crossfades), memory (resident splats, decoded chunks held) and cost (splats uploaded, `select`/`apply` time, draw GPU
time). With `quality=N` it also scores N frames against an oracle stream that shows every region at its finest LOD
with room for all of it, as PSNR over content pixels (pooled, not averaged in dB). `set=name:value,...` overrides
`StreamSettings` fields, `capacity=` sizes the pool, and `dump=<dir>` writes each live and oracle frame as a PNG.
Look at the frames: the oracle is only as good as its own stream, and the first version of it silently loaded
7 of 223 leaves. A pool cannot exceed about 11 million splats on a default device (one 128 MiB storage binding).

### GPU memory

`stream_bench` prints the pool's real GPU memory from the allocator (`memreport=1` lists every allocation). A
streaming pool costs 48.8 bytes a splat: centres 12, three RGBA8 splat textures 12, the sort's scratch (depth keys
4, one more key buffer 4, two value buffers 8), per-slot fade 4 and LOD 4. It used to cost 76. Four changes took
the other 27 bytes off, with no change to any pixel or streaming decision (the same benchmark run scored the same
38.1 dB):

- The second key ping-pong buffer is the depth-key buffer, which is free scratch after the first radix pass.
- The scene draws from the sorter's own result buffer instead of copying it into a separate `splatOrder` buffer
  (`DepthSorter::indices_buffer`).
- A pool for a level without spherical harmonics keeps a 1x1 stand-in for the per-slot SH labels
  (`scene::pool_for_level`).
- A pool's NaN centres and unit fades are filled by GPU copies from a 1 MiB pattern. `write_buffer` and a buffer
  mapped at creation both stage a second full copy until the first submit, a transient 16 bytes a slot that set the
  peak.

The decoded-chunk cache is not a peak-memory lever: with the 6M pool, `cache_chunks` 16, 8 and 4 hold 21, 14 and 10
chunks on average but peak at about 329, 318 and 274 MiB, because chunks that are needed or prefetched dominate the
peak, while the decode work grows 1.7x and 3x. `select_share_percent` (the pool share the selection may fill,
default 85) does not buy quality either: 85, 92 and 97 all score 38.1 dB while the pool's peak occupancy goes from
85% to 99%; a higher share trades fewer LOD swaps for slightly more holes.

### Hot-path audit

A read-through of the per-frame path (browser `frame`, scene, sorter, streamer, post-processing, UI, shaders) looked
for work that is unneeded, repeated, or done with a poor algorithm. Findings were checked against the code and
measured with `splat_bench` where a GPU effect was claimed (interleaved runs, 8.9 M-splat scene, 1280x720):

- Measured, no gain (do not retry): reading the debug-LOD label only after testing the debug mode (identical, 8.1-8.4
  ms both ways); hoisting `matrix_view * matrix_model` out of the vertex shader (upper bound measured by dropping the
  product, 8.1-9.1 ms against 8.3-8.9): the driver already hoists uniform-only math.
- Measured, real but bounded: the impact-decal depth prepass is a second full splat draw while any bullet mark or
  smoke is alive: +1.5 to +2.3 ms on about 10 ms (`splat_bench ... depth`). The depth pass already uses alpha clip
  0.5. Since done: the prepass now draws only the splats nearer than the farthest impact (see "Depth prepass range"
  below).
- Negligible at current sizes: the streamer's per-frame CPU is about 0.03 ms median for `select` and `apply`
  (`stream_bench`, 1,700 leaves), so its O(leaves) scans, the `evict` rebuild, and `requests_up_to` do not matter
  until levels have tens of thousands of leaves. The Rust side of a frame is about 0.8 ms, half of it the UI
  (extract 0.1 ms, render 0.3 ms); Lua is the larger CPU cost (1-3 ms).
- Not done, plausible: a still camera still re-sorts during each 0.45 s LOD crossfade (about 4 ms of GPU, only while
  standing still) although only the fade shares change; skipping the radix passes when no share crosses zero would
  avoid it. The sort also covers culled slots, which a compaction step could skip (sort time would then follow the
  visible fraction); that is the largest remaining GPU item but needs a deterministic, stable compaction.

### SIMD on the CPU hot path

The one real per-splat CPU loop is the streamer's upload encode (`stream/upload.rs`, up to 250,000 splats per frame):
re-quantise each centre to the shared range, remap the scale and colour bytes, and build labels. It is now written as
fixed-size block loops (`stream/simd.rs`, 64 splats) with no per-texel bounds checks, so LLVM emits NEON on aarch64
and `simd128` on wasm32; `.cargo/config.toml` enables `+simd128` for `wasm32-unknown-unknown` (every WebGPU browser
has it). No hand-written intrinsics: the portable form is bit-identical on every target and cannot drift.

- Rounding is written as trunc plus a `>= 0.5` compare because wasm has no round-half-away instruction. A block whose
  rescaled value is NaN, negative or negative zero (the cases the scalar code sends down its `direct` path) is detected
  up front and redone with the scalar expression, so the vector path never has to agree on edge cases.
- The 65,536-entry decoded-centre lookup is a gather, which no portable vector ISA has; it stays scalar over
  precomputed indices and is now most of the remaining time (about 2.9 of 3.8 ns a splat, three L2 lookups).
- Measured natively (aarch64, 218k real Nelson splats): 6.4 ns a splat before, 3.8 ns after (1.4 -> 0.82 ms);
  `stream_bench` apply p99 1.6 -> 0.89 ms. The wasm build was checked for instantiation, correct rendering of both
  a streamed (Nelson) and a single-bundle (Barn) level, and the presence of SIMD instructions, but the wasm speed-up
  itself was not isolated (no wasm runner here), so treat the native figure as the evidence.
- `sog::centres` (single-bundle level build) now decodes through a per-axis 65,536-entry table, evaluating `exp` 196k
  times instead of three times per splat; results are bit-identical (tested at 70,000 splats).
- Tests: `vectorised_centres_match_the_scalar_reference_with_a_lookup_table` (all block boundaries, NaN, infinite,
  negative and signed-zero ranges) and `table_decoded_centres_are_bit_identical_to_the_direct_expression`.
  `benchmark_centre_stage_share` and `benchmark_reused_upload_buffers` (`LOD_UPLOAD_LEVEL=<absolute level dir>`) are
  the manual benchmarks.

### CPU cache behaviour

A pass over the CPU code for cache-unfriendly access, measured warm (data cached) and cold (64 MiB streamed through
the caches first, as the renderer and scripts do between queries). Results must be bit-identical; every benchmark
below prints a checksum or compares against the previous code.

- Voxel octree lookups (`Octree::block_mask_with` + `BlockCursor` in `svo.rs`): every block lookup used to walk all
  `tree_depth` levels from the root (9 dependent reads on the lobby's 6.4 MB tree). The cursor keeps the previous
  walk's node at each depth and resumes from the deepest node the new block shares with the old one (the highest
  differing bit of the block coordinates gives that depth), so neighbouring blocks cost one or two reads. Rays,
  ground probes, capsule slabs, minimap slices and the debug voxel view use it; a wide slab (a minimap slice spans
  360 blocks) now gets a heap block cache instead of re-reading a block for every 4-cell row segment.
  `block_cursor_matches_the_root_walk_in_any_lookup_order` checks it against the root walk on random trees (solid
  nodes, early mixed leaves, absent children) and the local levels' trees in raster, reverse, random-jump and
  random-walk orders. `voxel_cache_bench` (interleaved against the previous build, lobby, 256 queries spread over the
  level, identical checksums): ground probes 0.79-0.94 to 0.19 ms warm and 0.85 to 0.35-0.49 ms cold, capsule moves
  7.9-8.3 to 5.2-5.9 ms, minimap slices 157-165 to 54-56 ms for 16; hitscan rays unchanged (they already reused the
  current block).
- Splat minimap (`SplatColumn` in `scene/src/minimap.rs`): each map request scanned every decoded centre (up to 3 M,
  12 bytes each) to keep the few percent within the slab's height. Centres are now bucketed by height once per chunk
  (a counting sort into up to 4096 buckets, 33 ms for 2.5 M) with heights in their own array, and a request reads
  only the buckets the slab overlaps (plus one either side) and applies the original test: 2.19 to 0.39 ms per
  request for 2.5 M centres. `height_bucketed_columns_draw_the_same_map_as_the_full_scan` compares the images,
  including non-finite heights and centres exactly on the slab boundary.
- Measured, not cache-bound (left as they are): the streaming upload's centre decode does three lookups per splat in
  65,536-entry tables (768 KB), but on real streamed chunks (Morton-ordered) forcing every index into the first 256
  entries changes nothing (2.24 against 2.22 ns a splat, `benchmark_upload_stages`), and its other stages are
  sequential streams. The ECS keeps components in `BTreeMap`s, but the gameplay scene has 241 entities (217 of them
  UI), which fit in cache whatever the layout. The collision queries' cold/warm gap was 3-19% before the cursor, so
  the remaining capsule cost is the per-cell penetration arithmetic, not memory.

### Flame-graph segments (sort, draw, post, CPU)

`docs/flamegraph/renderer-flamegraph.html` profiles a frame in the browser (`?stack_profile=1`): the GPU is the
limit, split between the depth sort (`splat_prepare`) and the splat draw (`scene_draw`); post-processing is about
2% and the page thread (about 3 ms) overlaps the previous GPU frame. **Absolute GPU times move with the GPU's clock
state between sessions (the unchanged static draw read 12.4 ms in one session and 6.0 ms in another), so only
interleaved A/B runs in one session count as evidence.** Natively `splat_bench` prints `drawn` (splats in view) and
`stream_bench ... hold=1` times a still and a turning camera at a fixed pose plus a settling phase.

- Sort, done: the depth key is a 16-bit logarithmic depth (1 mm to 100 km in 65,536 steps, 0.028% of the depth a
  step, finer than the old float key's 0.05%), so the radix sort takes four passes instead of five. In indirect mode
  the key pass also compacts the splats in view in slot order (`scan_groups` + `compact` in `depth_keys.wgsl`) and the
  radix passes read their count from the GPU (`USE_INDIRECT_SORT`), so culled slots are never sorted. While the camera
  is still and a crossfade only changes shares (same ranges, none crossing zero), the sort is kept and only the shares
  are rewritten (`fade_updates` in the sorter profile). Native A/B (`splat_bench`, lobby, spinning): 10.95 to 9.86 ms.
  Browser A/B of compaction alone (`?sort_compaction=0`): within noise on Nelson and the Barn, where the streamer
  keeps few culled slots. Settling at a still camera (`stream_bench hold=1`): 72 of 149 sorts become share rewrites.
- Correctness: a compacting sort defines its order only for the splats in view, so the vertex stage bounds the draw
  by `splatVisible` (the GPU count; all ones for a sort that does not compact) inside the last partial instance.
  `compacted_and_full_sorts_draw_the_same_picture` renders both ways pixel-identically (and fails without the bound);
  `share_only_fade_updates_match_a_full_sort` covers the fade path.
- Draw, measured and left: by elimination experiments the draw is about 20% fetch floor, 15% covariance, 40% colour
  (spherical harmonics about 20%) and 25% raster and blend. A per-splat projection cache (the vertex code run once per
  splat in a compute pass, the vertex stage only placing corners) was built and measured slower natively (3.0 to
  4.0 ms static): the per-splat work is fetch-latency-bound and the four corner invocations already share it.
  Removing the SH codebook lookups entirely saves at most 0.4 ms of 7.9, not worth doubling the SH palette's memory.
  Reducing drawn splats is the remaining lever, and that is an image-quality trade.
- The browser and native viewer request the high-performance adapter, which selects the discrete GPU on machines
  that have two.

#### Where a moving-camera frame goes (lobby, 8.9 M splats)

`splat_bench` has `wobble` paths that alternate the yaw by 0.05 degrees, so they re-sort every frame while drawing the
same view as the matching static path, and `COUNTERSPLAT_TIMESTAMPS=1` splits each frame into GPU spans: the sort's
stages (`sort::set_stage_timer`, which splits the sort's compute pass only while set), the depth prepass and the
colour draw. `COUNTERSPLAT_BENCH_PATHS=wobble,turned` runs a subset. On the development machine (integrated NVIDIA
GPU, 1421x1266, `Rgba16Float` scene target as in the browser), looking 90 degrees from spawn, where 6.3 M splats pass
the coarse view cull:

| span | GPU ms |
|---|---|
| depth keys (all 8.9 M slots) | 1.06 |
| compaction | 0.46 |
| four radix passes | 0.77 + 0.67 + 0.61 + 0.52 |
| colour draw | 13.1 |

The draw is mostly per splat, not per pixel: the same view at a quarter of the pixels still drew in 10.5 of 14.1 ms.
The re-sort added 4.5-4.8 ms, which is what pushed this view past 16.7 ms while turning.

#### Footprint cull in the sort (done)

The coarse view test in the key pass keeps any centre within 1.5x the screen, and the vertex stage then discards
splats whose footprint is under `minPixelSize` or off screen at its full extent, after the sort has ordered them and
each has cost four vertex invocations. The key pass now reads each splat's scale (the same RGBA8 codebook indices and
codebook the vertex stage reads) and leaves out every splat the vertex stage is sure to discard (`footprint_culled` in
`depth_keys.wgsl`): its projected covariance's largest eigenvalue is bounded by `s_max^2 |W|^2 |J|_F^2` (largest axis
scale, the view-model's stretch by power iteration, the perspective Jacobian's Frobenius norm), plus the 0.3 dilation,
plus depth-of-field variance (computed exactly per splat) and the most motion blur can add (1024/12) when those
effects are on, with a few percent of margin for the vertex stage's half-precision scales. A splat the bound says is
too small, or off screen even at the bound's extent, draws nothing, so the picture is unchanged.

- Exactness: `footprint_cull_leaves_out_splats_without_changing_the_picture` (20,000 random splats with scales from
  0.1 mm to 1 m, three views including a non-square one and one inside the cloud, with and without depth of field and
  motion blur) and `cargo run --release -p countersplat-render --example cull_verify -- assets/levels/<id>` (the real
  level, six views) compare every byte with the cull off and on: identical. On the lobby the cull left out 19.5%
  (spawn), 22.4% (90 degrees), 70.4% (180 degrees), 26.4% (270 degrees) and 18.8% (90 degrees with both blurs) of the
  splats the coarse test kept. `cull_estimate` is the CPU-side estimate that motivated it.
- Native A/B (interleaved, lobby; another app was using about 40% of the GPU, so the spread is wide): turning at 90
  degrees 19.5/18.9 to 17.1/17.6 ms; still at 90 degrees 13.2/13.5 to 12.4/12.7 ms; still at 180 degrees 6.4/6.2 to
  5.5/5.4 ms; the spawn view within noise. The key pass costs about 0.1 ms more for reading the scales; the radix
  passes and the draw shrink with the splats left out.
- Switches: `?footprint_cull=0` in the browser, `COUNTERSPLAT_FOOTPRINT_CULL=0` natively.

#### Sort: idle radix workgroups and unchanged fade writes (done)

- The radix passes are dispatched for every active slot, but a compacting sort only has the splats in view to order.
  A histogram or reorder workgroup wholly past the GPU's visible count now stops at once (`skip_empty_workgroups` in
  `sort.rs`: the count is read with `workgroupUniformLoad` so the early return stays in uniform control flow; a
  skipped histogram workgroup still writes zero counts, because its block-sum slots hold the previous pass's scanned
  offsets). Switch: `COUNTERSPLAT_RADIX_SKIP=0`.
- The key pass wrote every slot's crossfade share into `fade` on every sort (4 bytes a slot, 36 MB on the lobby). A
  compacting sort never draws a culled slot, so it now writes every slot's share (culled and empty slots included)
  once after `set_fades` changes them, and nothing on later sorts (`fade_mode` in `depth_keys.wgsl`; direct sorters,
  which draw every slot, still zero culled ones). Empty slots get their share too, so a splat uploaded into one later
  without a crossfade is not left transparent; `streamed_splat_renders_and_respects_the_active_prefix` sorts the empty
  pool first to cover that, and fails if empty slots are written as 0. Switch: `COUNTERSPLAT_FADE_SKIP=0`.
- `cull_verify` also renders the level with a direct, uncompacted sort as an independent reference: byte-identical in
  all six lobby views at 4-bit and 8-bit digits.
- Native A/B (three interleaved rounds, re-sorting every frame, with the footprint cull on; another app was using about
  40% of the GPU): at the spawn view (1.3 M in view) depth keys 0.89 to 0.74 ms and the radix passes 0.83 to 0.37 ms;
  at 90 degrees (4.9 M in view) keys 0.98 to 0.82 ms and radix 1.89 to 1.81 ms, the frame 19.8 to 19.2 ms. 8-bit
  digits on top were still no better (radix 0.62 ms at spawn, 1.90 ms at 90 degrees, its first pass alone 1.4 ms),
  so 4-bit stays the default.

#### Group skip in the key pass (done)

The key and compaction passes read every active slot even when most are outside the view. Each 256-slot group (one
key-pass workgroup) now has a box around its centres, stored after the per-group counts in `group_counts` (six f32
words a group, which keeps the pass within WebGPU's default of eight storage buffers per stage). `group_outside` in
`depth_keys.wgsl` skips a group when all eight box corners fail the same plane of the coarse view test, each by a
margin of 1e-5 of the summed term magnitudes (far above the per-splat evaluation's rounding), so every splat inside
would have been culled; the group then reads no centres and writes no keys, and `compact` returns early for any group
with nothing in view (its keys are stale). Uploads, moves and releases mark their slot range (`invalidate_slots`),
and the next sort recomputes those groups' boxes first (`group_bounds`). Slots are spatially coherent: on the lobby
whole groups hold 80.9%, 27.3% and 81.0% of the slots at spawn, 90 and 180 degrees, against 81.8%, 29.2% and 83.4%
culled per splat (`cull_estimate`).

- Exactness: `cull_verify` matches the uncompacted full sort byte for byte in all six lobby views;
  `group_skipping_follows_uploads_moves_and_releases` streams visible splats into slots that held an off-screen
  cluster, moves and releases them, and compares every step against a pool that sorts every slot (it fails, 1,764
  bytes off, if slot invalidation does not refresh the boxes).
- Native A/B (three interleaved rounds; the GPU was shared with another app at about 64%, so only GPU spans are
  meaningful): depth keys 0.69 to 0.29 ms at spawn, 0.75 to 0.50 ms spinning, 0.79 to 0.67 ms at 90 degrees;
  compaction 0.18 to 0.14 ms at spawn. Switch: `COUNTERSPLAT_GROUP_CULL=0`.
- In one quiet moment of that run the 90-degree view while re-sorting took 12.7 ms with an 8.6 ms draw, so on an
  unshared GPU the worst lobby view is under the 16.7 ms budget.
- After this every sort stage moves its data at about 195-245 GB/s on this machine, so the sort is at the memory
  bandwidth floor for what it reads and writes; fewer passes (8-bit digits) scatter worse and lose.

Also measured: the gameplay exposure path in the vertex stage (gamma to linear, scale, back to gamma, then decoded
again for the linear target) costs 1-2% of the draw (static 5.83 against 5.92 ms, 90 degrees 17.13 against 17.29 ms,
exposure off and on), within noise, so it was left as it is. The scene's colour hook now reads the per-slot LOD label
only when a LOD debug view is on, instead of on every vertex.

#### Depth prepass range (done)

The impact prepass draws only the splats nearer than the farthest impact: after the sort, `depth_range.wgsl` binary
searches the sorted order (farthest first) for the first splat no farther than that depth (with 0.2% margin) and
writes the prepass's start and indirect draw arguments; the prepass vertex stage offsets its order index by the
start. `depth_prepass_range_keeps_every_depth_nearer_than_the_limit` checks the depth buffer is bit-identical
wherever the full prepass is nearer than the limit. Native A/B on the lobby at 90 degrees (6.3 M in view): no
prepass 13.8/14.1 ms, full prepass 16.2/16.6, limited to 15 m 13.2/13.1, limited to 5 m 14.2/14.1, so a limited
prepass costs about nothing where the full one cost 2.3-2.5 ms; at the spawn view the full prepass cost about 0.3 ms.

#### Measured and rejected (do not retry without a new idea)

- 8-bit radix digits in the same multipass sort (`COUNTERSPLAT_RADIX_BITS=8`, two passes over the 16-bit key instead
  of four; same order, all sort tests pass): radix time 2.52 to 2.14 ms with 6.3 M splats in view but 0.83 to 1.30 ms
  with 1.6 M, because the 256-bucket scan is sized by the active slots, not the splats in view. Mixed, so the default
  stays 4-bit. Retried after idle workgroups were made to stop early (above): still no better than 4-bit; its first
  pass, which scatters to 256 buckets, costs 1.4 ms alone at 4.9 M splats in view.
- Triangle splats (one triangle around the ellipse instead of a two-triangle quad): spin 7.24 to 6.89 ms in RGBA8,
  but static 5.61 to 5.77 ms in the browser's `Rgba16Float`: more covered pixels cost more than the saved vertex.
- Colour computed once per splat at one corner (`colour corner`): no change, the corners diverge within a wave.
- `Rg11b10Ufloat` scene target instead of `Rgba16Float` (which costs about 15% over RGBA8, static 5.61 against
  4.88 ms): 29-32 dB PSNR with visible banding in dark gradients. Rejected on image quality.

### Drivers the tests have run on

The render tests (`cargo test -p countersplat-render`) accept `COUNTERSPLAT_BACKEND` (`vulkan`, `dx12`, `gl`) and
`COUNTERSPLAT_ADAPTER` (part of an adapter's name, or `warp` for the software renderer) to run the same suites on
each driver of a machine; `cargo run -p countersplat-render --example adapters` lists them. On the development
machine (an integrated NVIDIA Blackwell GPU) all suites pass on Vulkan, DX12, the WARP software renderer, and OpenGL
when run with `--test-threads=1` (OpenGL hangs when many tests create devices on parallel threads, one test at a
time passes). That includes indirect draws and the one-million-splat sort. It is still one hardware vendor.

Known flake: with parallel test threads the `countersplat-render` unit tests and `render_smoke` occasionally stall for
good (about 1 run in 16 on the development machine, whose GPU is shared with other GPU applications): several
unrelated GPU tests are then blocked at once. Run single-threaded (`-- --test-threads=1`) and it did not happen in 28
runs. It was seen before the memory work above as well, so it is not caused by it, but its cause is not known; use
a timeout around parallel GPU test runs.

## Extension rules

Add game-specific actors, input, UI, and effects in application or feature crates that depend on these libraries.
Pass data through public types instead of reaching into another crate's files. Keep format decoding independent of
GPU and physics backends. Add a narrow adapter at the application boundary when two systems need to collaborate.
Test cross-system behavior at that boundary, including a browser build when changing portable APIs.
