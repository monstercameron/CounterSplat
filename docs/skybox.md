# HDR skybox scene layer

A `Skybox` is a scene component that references a host-owned environment asset by opaque ID. It contains no GPU
handles, file paths, Lua VM, or image bytes. `countersplat-scene` defines the data; `countersplat-scene-ecs`
registers a validated ECS adapter and extracts an owned snapshot; `countersplat-assets` decodes Radiance HDR; and
`countersplat-render::env` converts panoramas to cube faces and draws them. A host connects these pieces.

The component contains `asset`, `enabled`, `priority`, `exposure` (linear multiplier, 0–64), and
`rotation_degrees` (world +Y). `Skybox::validate` checks asset IDs and numeric ranges. Multiple skybox entities are
allowed: `scene_ecs::extract` chooses the enabled valid component with the greatest priority; equal priorities use
the lowest entity ID. If the selected asset is not resident, the host decides how to handle it. The browser keeps
the existing sky until a replacement has decoded and uploaded successfully.

## ECS and Lua

```rust
use countersplat_ecs::World;
use countersplat_scene::Skybox;

let mut world = World::new();
countersplat_scene_ecs::register(&mut world)?;
let mut component = Skybox::new("outdoor.sunset")?;
component.exposure = 1.2;
component.rotation_degrees = 35.0;
let entity = countersplat_scene_ecs::spawn(&mut world, component)?;
let selected = countersplat_scene_ecs::extract(&world).unwrap();
assert_eq!(selected.entity_id, entity.to_string());
# Ok::<(), Box<dyn std::error::Error>>(())
```

When the host registers the adapter on the world used by its Lua bridge, scripts can use the generic ECS API:

```lua
local sky = ecs.spawn()
ecs.set(sky, "Skybox", {
    asset = "outdoor.sunset", exposure = 1.2,
    rotation_degrees = 35, priority = 10, enabled = true
})
local settings = ecs.get(sky, "Skybox")
settings.rotation_degrees = 90
ecs.set(sky, "Skybox", settings)
```

Invalid script writes return an error without replacing the previous value. The native Lua integration test runs
this bridge against a real ECS world. The browser uses its Lua 5.4 host against the shared gameplay ECS world,
which includes scene, player, level, and UI entities.

## Asset and renderer contract

1. Fetch bytes in the host or asset worker. Decode them with `countersplat_assets::environment::decode_hdr`. It
   accepts Radiance RGBE `.hdr` panoramas, checks the 2:1 dimensions before allocating, caps encoded data at 64 MiB
   and pixels at 16,777,216, and rejects non-finite or negative values. EXR is not currently supported.
2. On a worker, call `countersplat_render::env::try_to_cubemap(&panorama, face_size, 0.0)` and, when transferring
   between workers, `cubemap_rgba16f`. Faces are +X, -X, +Y, -Y, +Z, -Z. Browser conversion is limited to a
   512-pixel face; the example uses 256.
3. On the GPU owner, cache a `Sky::try_new` from a validated `Cubemap` or `Sky::from_rgba16f` from transferred bytes,
   keyed by the same opaque asset ID. The browser example limits its cache to eight IDs and reuses one ID for local
   replacements. Asset failures leave the prior selection intact.
4. Each frame, extract the scene snapshot on the world owner and send the owned value to the renderer if needed.
   Resolve its asset ID in the host cache and call `Sky::draw_rotated` with its exposure and rotation in a cleared
   color/depth pass **before** drawing splats. The sky is at the far plane and does not write depth. Draw UI after
   the scene. Runtime rotation changes a shader uniform; it does not re-project or re-upload the HDR.

The preview reads skybox entities and asset references from [gameplay.json](../assets/scenes/gameplay.json); see [scene JSON](scenes.md).
The example UI sends only changed controls to the scene owner, and reflects scene settings back into the widgets.
Its exposure and rotation sliders expose a narrower range than the underlying component. In the browser preview,
HDR loading and conversion run in a dedicated Web Worker; dragging a `.hdr` file onto the canvas replaces the sky
locally. In the native window, use `--scene=path/to/scene.json` to select a manifest, `--sky=path/to/file.hdr`, `--sky-exposure=N`, `--sky-rotation=DEGREES`, or
`--no-sky`; **S** toggles visibility. The bundled CC0 example is used by default when launching from the project root.

This is a visible background layer, not image-based lighting: it does not yet light splats or other objects, create
reflections, or remove sky geometry already baked into a scan. HDR radiance is tone mapped for the display target.
