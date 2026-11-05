# Display resolution and viewport scaling

Open **Display** above the canvas to select a render resolution. Presets cover
720p, 1080p, 1440p, 4K, 4:3, and ultrawide sizes; **Custom** accepts independent
width and height. **Viewport (native)** follows the canvas dimensions and
browser device-pixel ratio, subject to the GPU dimension and 16.8-megapixel
limits. The setting persists in browser-local preferences and does not rewrite
the scene JSON.

With no saved preference the scene follows the window (**Viewport (native)**) instead of a fixed 1280×720: a
fixed-size scene is scaled up to fill any larger or high-DPI canvas, which softens the whole picture (a 1112×960
window at a device-pixel ratio of 1.5 is a 1668×1439 canvas, so 1280×720 was stretched 1.3× across and 2× down). A
viewport-sized scene is scaled down, keeping its aspect, to at most `MAX_VIEWPORT_SCENE_PIXELS` (1.8 megapixels,
`crates/scene/src/display.rs`) because frame cost grows with pixels, about 9 ms a megapixel in the browser on the
development GPU; a fixed resolution is never capped. The UI is drawn at full viewport resolution either way.

The canvas output fills the available browser viewport. The game scene renders
at the selected resolution and retains that resolution's aspect ratio:

- **Fill viewport** scales proportionally and crops the center image at its edges.
- **Fit entire image** keeps every scene pixel visible and adds black bars as needed.

The ECS UI is rendered separately at viewport resolution after the scene is
presented, so panels and pointer coordinates remain aligned and visible in both
modes. The 3D view, first-person footage, depth, bloom, and image correction use
the selected scene resolution; the UI stays sharp when a lower scene resolution
is scaled up. The browser host validates sizes against the active device and a
bounded pixel budget before allocating render targets.

`countersplat-render::postprocess::ViewportScale` implements the reusable
aspect-preserving presentation math. The native viewer retains its existing
window-sized render behavior; these display controls currently belong to the
browser host.
