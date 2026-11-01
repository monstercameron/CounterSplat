# CounterSplat

CounterSplat is a reusable Rust foundation for applications built around Gaussian-splat scans. It extracts the scan
formats, GPU renderer, streaming, voxel geometry, physics primitives, ECS, and timing from NeuralVisionRacer into a
single workspace. Game rules, vehicles, HUDs, and game-specific configuration live outside these libraries.

The current deliverable is an engine foundation with a native inspection tool and a live browser WebGPU canvas.
The canvas streams a scan and supports a Lua-controlled free-flying player camera; it is not yet a complete interactive game.

## Workspace

| Package | Responsibility |
| --- | --- |
| `countersplat-audio` | Layered stereo mixing, playback, native streaming, and owned commands. |
| `countersplat-audio-ecs` | Script-visible audio scene components and owned ECS snapshots. |
| `countersplat-audio-runtime` | A bounded native audio owner thread and device startup. |
| `countersplat-core` | Rational fixed-step clock, frame scheduling and adaptive pacing, and structured logs. |
| `countersplat-ecs` | Entity/component storage, systems, hierarchy, events, snapshots, and script-neutral commands. |
| `countersplat-scene` | Versioned scene JSON, resource catalogs, and renderer-independent scene components. |
| `countersplat-scene-ecs` | Validated scene-layer instantiation, ECS/Lua skybox adapters, and owned snapshots. |
| `countersplat-ui` | Renderer-independent UI elements, input, events, and frame-cadence measurements. |
| `countersplat-ui-ecs` | Validated UI components, hierarchy extraction, and script-facing UI events. |
| `countersplat-ui-wgpu` | egui-based fonts, shapes, and controls composited onto a wgpu scene. |
| `countersplat-scripting-lua` | Thread-owned Lua 5.4 VM and data-only ECS bridge. |
| `countersplat-runtime` | Named subsystem workers, bounded queues, reply tickets, and latest-value snapshots. |
| `countersplat-assets` | SOG v2, level-of-detail metadata, sparse voxel octrees, and optional camera hints. |
| `countersplat-shaders` | WGSL source catalogue, preprocessing, and Gaussian-splat shader composition. |
| `countersplat-render` | GPU scene upload, depth sorting, streaming pools, camera math, and sky rendering. |
| `countersplat-physics` | Game-neutral Rapier bodies and scan-derived floor and obstacle colliders. |
| `countersplat-viewer` | A native CLI with separate asset-loading and GPU-rendering workers. |
| `countersplat-web` | Browser WebGPU canvas host with scan decoding in a Web Worker. |
| `countersplat-threaded-demo` | A native ECS, Lua, and physics worker integration example. |

Dependencies point toward the lower-level crates; no crate includes another crate's source files. See
[architecture](docs/architecture.md) for the dependency graph, data flow, and extension rules, and
[migration notes](docs/migration.md) for what was carried over. See [threading](docs/threading.md) for the
worker design, current integration, and browser constraints. See [timing](docs/timing.md) for fixed ticks,
frame deadlines, adaptive pacing, and host integration. See [UI](docs/ui.md) for controls, ECS/Lua hooks,
frame counters, and the rendering contract. See [skyboxes](docs/skybox.md) for HDR assets, ECS selection,
and host integration. See [audio](docs/audio.md) for layered mixing, stereo and ECS/Lua hooks. See [scene JSON](docs/scenes.md) for entity, component, asset, and script definitions, and [asset loading](docs/asset-loading.md) for preload policies and level transitions. The [4DGS research](docs/4dgs-research.md) records a proposed path for dynamic splat playback.

## Build and test

Install Rust 1.96 or newer, Python 3.11 or newer, and a working native C compiler (used to build vendored Lua 5.4). From this directory:

```powershell
cargo test --workspace --locked
python scripts/check_boundaries.py
python scripts/test_preview.py
```

GPU tests use a compatible adapter when available and explicitly report when they cannot run. To check the library
crates for the browser target, install it with `rustup target add wasm32-unknown-unknown`, then run:

```powershell
cargo check --locked --target wasm32-unknown-unknown -p countersplat-core -p countersplat-ecs -p countersplat-runtime -p countersplat-assets -p countersplat-shaders -p countersplat-render -p countersplat-physics -p countersplat-web
```

The Lua crate is a native implementation and is not included in that browser check. `countersplat-web` supplies a
separate browser canvas host; it does not yet include the native Lua or Rapier worker integration. The browser demo owns its UI ECS world locally;
the browser runs its UI and player scripts through a separate Lua 5.4 backend.

## Subsystem threading example

```powershell
cargo run -p countersplat-threaded-demo
cargo run -p countersplat-threaded-demo -- --realtime 3
```

These examples run a Lua VM, an ECS world, and a Rapier world on three separate owner threads. The real-time mode
drives the workers from a 60 Hz monotonic simulation clock and reports software frame requests. Lua creates an entity, physics
simulates a body, and the updated position returns through ECS to Lua. The viewer separately runs asset decoding and
GPU rendering on their own workers. These are working native examples, not yet a single real-time host tying all five
subsystems together. See [the demo](apps/threaded-demo/README.md), [ECS](crates/ecs/README.md), and
[Lua API](crates/scripting-lua/README.md) for usage and integration details. The proposed
[item and interaction API](docs/item-interactions.md) is a design for future work, not an implemented feature.

## Inspect and render

No scan assets are bundled. An example CC0 HDR sky is included separately. Supply a SOG v2 file, an unpacked SOG chunk directory, or a level directory containing
`lod-meta.json` and its chunks:

```powershell
cargo run -p countersplat-viewer -- inspect "path\to\scene.sog"
cargo run -p countersplat-viewer -- inspect-level "path\to\level"
cargo run -p countersplat-viewer -- render "path\to\scene.sog" "preview\frame.png"
cargo run -p countersplat-viewer -- render-level "path\to\level" "preview\frame.png" 300000 z180
cargo run -p countersplat-viewer -- live-level "path\to\level" "preview\frame.png" 10 300000 z180
```

The optional final `z180` rotates splat data 180 degrees around Z for scans with the source project's y-down
convention. Omit it for unrotated data. `300000` is the example GPU pool budget, not a fixed engine limit. When
present, `viewer-settings.json` supplies the initial camera; otherwise the tool frames the metadata bounds.
`live-level` runs a finite, clock-driven offscreen preview and coalesces requests when the renderer is busy.

## Native window and VSync

The native window uses the same splat renderer and shows its active mode in the title bar:

```powershell
cargo run -p countersplat-viewer -- window-level "path\to\level" --z180 --vsync=on
cargo run -p countersplat-viewer -- window-level "path\to\level" --z180 --vsync=off --fps=120
cargo run -p countersplat-viewer -- window-level "path\to\level" --z180 --vsync=off --fps=monitor
```

Press **V** to toggle VSync, **F** to cycle off-mode caps (including unlimited), **M** to cap off-mode FPS to the
current monitor, and **Esc** to close. Use `--fps=unlimited` for no software cap, or `--fps=N` for a cap from 1 to
1000. `--capacity=N` sets the splat pool. Immediate presentation must be supported by the native surface; the
viewer reports an unsupported request rather than silently selecting another mode. GPU submissions are bounded to
one in flight. The standalone native window does not yet include the browser's ECS overlay.

When launched from the project root, the native window loads `assets/scenes/gameplay.json` and its configured HDR sky. Use `--scene=path/to/scene.json` for another manifest. Press **S** to show or hide it.
Use `--sky=path/to/file.hdr` to choose a Radiance panorama, `--sky-exposure=1.2` or `--sky-rotation=90` to adjust
it, or `--no-sky` to disable loading. The HDR is converted on the native asset worker.

## Live browser canvas

The browser host opens an ECS level selector on the WebGPU canvas. Click a local level and press **Launch**;
use **Levels** in gameplay to return to the selector. Use WASD, E/Q, and Left Shift to move, and drag the scene to
look; see [player controls](docs/player-controls.md) for remapping and camera settings. The selector reads `assets/levels/catalog.json`,
`assets/scenes/level-selector.json`, and `assets/scenes/gameplay.json`.

The in-canvas frame-pacing panel offers an optional
FPS cap; its uncapped request cadence follows browser animation callbacks. The browser controls presentation and
cannot provide true VSync off. Its fixed-step clock runs from animation callbacks;
scene fetching and SOG decoding run in a Web Worker, and the renderer limits frames in flight. An ECS-owned UI
layer draws controls and a submitted-frame FPS / frame-time counter over the scan. A scene ECS skybox layer draws
behind the splats; its HDR decode runs on a separate worker. Use the sky panel to change exposure or rotation, or
drop a local Radiance `.hdr` file onto the canvas to replace the example. A WebGPU-capable
browser and a local level catalog under `assets/levels` are required. Scan data is distributed separately.

```powershell
# Once, if the wasm target and matching binding tool are not installed:
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked

# Rebuild the browser package after changing Rust code:
.\scripts\build_web.ps1

# Serve the selector and local asset catalog:
python scripts/preview.py
```

Edit the scene JSON files under `assets/scenes` and refresh the browser to apply scene, UI, and resource changes. Open <http://127.0.0.1:18763/>. The generated `preview/pkg` bindings are build artifacts. The page renders through
WebGPU into `<canvas>`; the native PNG commands above remain optional offline exports. See
[timing](docs/timing.md) for what the displayed submitted-frame rate does and does not measure.

## Source and assets

The port starts from NeuralVisionRacer commit `87e95586eef89acae852d75a92d69b9d8dc5745c`. See
[third-party notices](THIRD_PARTY.md) for the upstream PlayCanvas shaders, blue-noise texture, and the small
metadata test fixture, and the example CC0 HDR sky. Scan licenses are separate; check them before redistributing scan data.
