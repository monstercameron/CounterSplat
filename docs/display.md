# Display resolution and viewport scaling

Open **Display** above the canvas to select a render resolution. Presets cover
720p, 1080p, 1440p, 4K, 4:3, and ultrawide sizes; **Custom** accepts independent
width and height. **Viewport (native)** follows the canvas dimensions and
browser device-pixel ratio, subject to the GPU dimension and 16.8-megapixel
limits. The setting persists in browser-local preferences and does not rewrite
the scene JSON.

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
