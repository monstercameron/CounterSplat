# Real-time 4D Gaussian splatting: feasibility for CounterSplat

Research assessment, October 2026. This is a proposed direction, not implemented functionality.

## What 4DGS means

Dynamic Gaussian splatting reconstructs a scene whose appearance changes over time and renders it from new viewpoints. It is not one universal interchange format. One family starts with 3D Gaussians and predicts their time-dependent deformation using a feature grid and a small neural network. [Wu et al.](https://arxiv.org/abs/2310.08528) report 82 FPS at 800 × 800 on an RTX 3090 for their tested scenes; their [reference implementation](https://github.com/hustvl/4DGaussians) also supports exporting 3D Gaussians at individual timestamps. A different family represents Gaussians in space and time directly; see [Real-time Photorealistic Dynamic Scene Representation and Rendering with 4D Gaussian Splatting](https://logosintelligencelab.github.io/4d-gaussian-splatting/).

Playback of prerecorded assets is plausible in real time. Capture, reconstruction, and training have different costs and are not established as real-time operations here. Published FPS figures are measured on specific scenes, hardware, resolutions and implementations; they are not predictions for CounterSplat, WebGPU, or mobile. Recent work such as [4DGS-1K](https://proceedings.neurips.cc/paper_files/paper/2025/hash/6c39d1a7eecfd98570d74bf7efec1be7-Abstract-Conference.html) reduces temporal redundancy and inactive work, illustrating why representation and workload matter as much as the headline frame rate.

## Fit with the current renderer

CounterSplat already has SOG decoding, CPU LOD selection, worker-based asset loading, a GPU splat pool, GPU depth sorting, WGSL drawing, and native/browser hosts. Those pieces provide a useful base, but current SOG scenes and LOD manifests are static; they do not establish a shared temporal format or stable Gaussian identities.

A moving splat must use the same animation timestamp for deformation, culling, depth keys, sorting, and drawing. The sorter caches results when camera/model inputs match: animation must call the existing `DepthSorter::invalidate` or otherwise include a geometry revision before reusing a sort. Static and dynamic splats that overlap transparently need a globally ordered representation or an explicitly accepted approximation; simply drawing two independently sorted layers can create incorrect blending. The current renderer draws the active pool prefix directly, so excluding inactive splats from the visible set alone does not eliminate all vertex work.

A suitable design keeps temporal metadata and clip references as renderer-neutral scene/ECS data. The host supplies a monotonic playback clock and schedules asset I/O. A renderer-owned animation stage updates GPU-resident data before depth-key generation and drawing. Native worker messaging and browser Web Workers remain host decisions. Dynamic scans do not automatically supply skeletal control, physical collisions, relighting, or game logic; use separate gameplay and collision representations where needed.

## Recommended milestones

1. **Playback prototype:** export a short prerecorded dynamic capture offline as independent timestamped 3D splat frames. Define a versioned sequence manifest with timestamps, frame asset IDs, bounds, and looping/seek policy. Convert only supported input, verify metadata and per-frame palettes, and cap CPU/GPU memory. Decode and prefetch a bounded number of frames on workers; start with nearest-frame playback and show buffering or a static fallback when data is late. This tests the real browser/native pipeline before committing to a new dynamic representation.
2. **Compact temporal representation:** after measuring the prototype, investigate stable-ID keyframes and deltas, with interpolation of position, log scale, orientation and opacity. Correctly handle births/deaths and never interpolate unrelated splats. Conservative motion bounds and time-aware LOD are needed for culling and prefetch.
3. **Native 4D or learned deformation:** if source assets and benchmarks justify it, add a versioned spatiotemporal format or port feature-grid/MLP evaluation to WGSL compute before sorting. CUDA-based research code is not directly usable as WebGPU shader code. Keep this implementation behind the same scene/asset interfaces, and check each upstream license and model/asset rights before adoption.

## Benchmark before promising a target

Use a small representative capture and fixed camera paths at 720p and 1080p. Compare the existing static scene to each dynamic approach on a discrete GPU, an integrated GPU, and intended mobile hardware. Record p50/p95/p99 frame intervals, GPU time for animation/sort/draw when supported, visible and active splat counts, overdraw, CPU/GPU resident memory, transfer bandwidth, startup and seek latency, stalls, dropped frames, and visual flicker against a reference. A 60 Hz frame is 16.7 ms total; 30 Hz is 33.3 ms. The current browser FPS widget measures submitted-frame intervals, not confirmed GPU execution or display presentation.

Check each adapter's storage-buffer and texture limits, memory budget, and optional timestamp-query support. WebGPU exposes compute and rendering but implementations and feature availability vary; use secure origins and capability checks, and retain a lower-quality or static fallback. See the [WebGPU specification](https://www.w3.org/TR/webgpu/).
