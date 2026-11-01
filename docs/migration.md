# Migration notes

The source is NeuralVisionRacer commit `87e95586eef89acae852d75a92d69b9d8dc5745c`. The source checkout remains
independent; CounterSplat does not import files from it at build time.

| Source | CounterSplat home |
| --- | --- |
| `splat-ecs` | `crates/ecs`, extended with script-neutral commands and snapshots |
| Engine timing and logging | `crates/core/src/time.rs`, `pacing.rs`, and `log.rs` |
| Renderer SOG and LOD parsing | `crates/assets/src/sog.rs`, `lod.rs`, and `codebook.rs` |
| Engine sparse voxel reader | `crates/assets/src/svo.rs` |
| Engine voxel and floor helpers | `crates/physics/src/voxel.rs` and `floor.rs` |
| Shader composition and upstream WGSL | `crates/shaders` |
| GPU scenes, sort, streaming, and sky | `crates/render` |
| Rapier integration | `crates/physics/src/world.rs`, exposed through game-neutral types |
| New scripting and worker infrastructure | `crates/scripting-lua` and `crates/runtime` |
| New integration examples | `apps/viewer`, `apps/web`, `apps/threaded-demo`, and `scripts/preview.py` |

The renderer retains SOG decoding, SH color, GPU sorting, LOD selection, streaming pool allocation, crossfades,
optional target cutouts, sky support, and native offscreen readback. Car lighting, wheel animation, racing effects,
the original game loop, dashboard, and vehicle mechanics are not part of this core extraction. Physics provides
scan-derived static floor and wall boxes; the original racing-specific streamed tile manager and handling are not
ported. The viewer offers offscreen commands and a native window, but is not yet a complete game.

There are no bundled playable scans. The browser host renders directly to a WebGPU canvas and generates its WASM
bindings with `scripts/build_web.ps1`. The native viewer can still export offscreen PNGs. The browser host is an
application layer rather than a dependency of the core, and does not yet integrate native Lua or physics workers.
