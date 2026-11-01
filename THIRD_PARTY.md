# Third-party and source notices

CounterSplat includes code derived from NeuralVisionRacer at commit
`87e95586eef89acae852d75a92d69b9d8dc5745c`, distributed under Apache License 2.0. See [LICENSE](LICENSE).

The WGSL catalogue under `crates/shaders/shaders` and the blue-noise texture under `crates/render/assets` come from
[PlayCanvas engine](https://github.com/playcanvas/engine), commit `209ce5752527ab5050ec9632ca6dc1272542b70d`,
Copyright (c) 2011-2026 PlayCanvas Ltd., under the MIT license reproduced in
[third_party/playcanvas/LICENSE](third_party/playcanvas/LICENSE). Individual shader headers retain their provenance.

`crates/render/tests/fixtures/lod-meta.json` is metadata from **Saburi alley - Yazd, Iran** by parsamahdavi48,
[SuperSplat scene](https://superspl.at/scene/d56e46e4), licensed under
[Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/). The metadata is
retained as a test fixture; the scene's image chunks are not included. A local preview may display a separately
provided scene, whose own license and attribution still apply.

The example `assets/images/environments/kloppenheim_06_puresky_1k.hdr` is **Kloppenheim 06 (Pure Sky)** by Greg Zaal
and Jarod Guest, provided by [Poly Haven](https://polyhaven.com/a/kloppenheim_06_puresky) under
[CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/). The browser build may package a copy under `preview/pkg/assets/images`.

The UI renderer uses [egui and egui-wgpu 0.37](https://github.com/emilk/egui), licensed under MIT or Apache-2.0. The MIT option is reproduced in
[third_party/egui/LICENSE-MIT](third_party/egui/LICENSE-MIT). The bundled default font package includes Hack, Ubuntu Light, and egui icon fonts. Their accompanying license and
attribution files are reproduced in [third_party/egui_fonts](third_party/egui_fonts/); these include the Ubuntu Font
Licence, SIL Open Font License, Hack/Bitstream notices, and icon-font MIT notice.

Audio uses [Kira 0.12](https://github.com/tesselode/kira) (MIT or Apache-2.0), [CPAL 0.18](https://github.com/RustAudio/cpal) (Apache-2.0), and [Symphonia 0.6](https://github.com/pdeljanov/Symphonia) (MPL-2.0). Their source and license texts are available in their upstream packages.

Rust dependencies and their licenses are identified in `Cargo.lock` and their upstream packages.

The browser Lua 5.4 runtime uses [Wasmoon 1.16.0](https://github.com/ceifa/wasmoon), MIT licensed; its distribution and license are in [third_party/wasmoon](third_party/wasmoon/). Lua is distributed under the MIT license by Lua.org, PUC-Rio.

The sparse-voxel sphere collision and iterative push-out approach in `crates/physics/src/voxel_collision.rs`
is adapted from [SuperSplat Viewer](https://github.com/playcanvas/supersplat-viewer), commit
`06d1c880edc4756a0ccf0a9f03f90fd1182ef178`, Copyright (c) 2011-2026 PlayCanvas Ltd., MIT licensed;
see [third_party/supersplat-viewer/LICENSE](third_party/supersplat-viewer/LICENSE).
