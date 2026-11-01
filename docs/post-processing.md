# Post-processing and image correction

The gameplay scene defines an `image_correction` entity with an `ImageCorrection` component in
`assets/scenes/gameplay.json`. Open **Image correction** at the lower right of the canvas, or select
**Image correction** in **ECS entities** and edit its exported component settings in the inspector.
The panel is collapsible and draggable. The scene preset uses a restrained filmic curve, slight
warmth and contrast, a soft bloom, and a mild vignette; it can be adjusted or bypassed entirely.

Controls include exposure in stops, contrast, saturation, warmth, tint, shadows, highlights, bloom
strength and threshold, vignette, grain, sharpening, and the filmic curve. **Reset cinematic grade** restores the
values authored in the current scene JSON. Sharpening ranges from 0 to 2; the gameplay preset starts at 0.2.
In the browser, edits persist locally across level changes
and reloads; they do not rewrite scene files. Camera lens exposure is applied earlier, so it combines
with the image-correction exposure.

## Render order

1. Splats, the HDR sky, and voxel debug geometry render into a linear RGBA16F scene target.
2. When bloom is enabled, exposed highlights are extracted with a soft knee and blurred in two
   separable half-resolution passes. Zero bloom strength skips those three passes.
3. A bounded four-neighbor sharpening filter enhances scene edges in linear HDR when its strength is above zero.
   The correction shader then combines bloom, exposure, white balance, tonal and saturation adjustments,
   an optional ACES-inspired filmic curve, vignette, and optional screen-stable grain. The result is
   written to the sRGB display target. The curve is a fit, not a full ACES color-management transform.
4. UI and menus are composited afterward, so the grade never changes their colors.

The intermediate scene target and two half-resolution bloom targets use approximately 12 bytes per
full-resolution pixel. They are recreated on resize. The source splats are photographic scans rather
than calibrated scene radiance; post-processing can improve their presentation but cannot recover
missing detail or replace lighting, composition, or art direction. Turning the filmic curve off can
clip HDR highlights on an SDR display.

## ECS, Lua, and hosts

`ImageCorrection` is a validated, renderer-independent scene component. Multiple components can
coexist; the enabled valid component with the highest priority is selected, with the lowest entity
ID breaking ties. Disabling every component bypasses the grade. `assets/scripts/image_correction.lua`
registers the inspector exports and handles the panel with normal ECS operations:

```lua
local id = ecs.get_resource("scene.entities").image_correction
local grade = ecs.get(id, "ImageCorrection")
grade.exposure_ev = -0.25
grade.bloom_intensity = 0.2
grade.sharpness = 0.4
ecs.set(id, "ImageCorrection", grade)
-- Optional: ask the browser host to persist this change locally.
ecs.emit("image.correction_changed", grade)
```

The browser and native window both consume an owned ECS snapshot and pass renderer-neutral values
to `countersplat_render::postprocess::PostProcess`. A host renders into `scene_view()`, calls
`encode()` to the output target, then draws UI. The native window currently reads scene JSON and has
no interactive Lua UI; the browser provides the live panel and local persistence. The separate
render passes allow additional effects to be inserted without coupling rendering to ECS or Lua.

Design references: [Epic's filmic tone-mapping and color-grading guidance](https://dev.epicgames.com/documentation/unreal-engine/color-grading-and-the-filmic-tonemapper-in-unreal-engine?lang=en-US)
and [Unreal's bloom overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/bloom-in-unreal-engine).
