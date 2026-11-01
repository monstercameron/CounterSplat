# Camera and lens

Open **Camera & lens** at the lower right of the gameplay canvas. The panel can be collapsed or dragged like the
other ECS panels. **View** changes the player's vertical field of view, yaw, pitch, and roll; mouse look and the
existing right-button optical zoom continue to work. **Reset view** restores the spawn pose, while **Reset lens**
restores the scene's camera-lens defaults.

**Standard lens** and **zoom lens** have independent iris, shutter, ISO, exposure, focus, sensor, depth-of-field,
and motion-blur settings. Select **Edit zoom lens** in the camera panel to tune the zoom profile; the field-of-view
slider changes to a zoom-magnification slider for that profile. With the box clear, the controls edit the standard
lens and its base FOV. **Reset standard lens** or **Reset zoom lens** resets only the selected profile; the latter
also restores the scene's zoom magnification.

Right-button zoom smoothly blends the numeric photographic settings alongside the optical magnification. The
depth-of-field and motion-blur switches use the selected profile after the blend crosses its midpoint. The UI
profile selector chooses what is being edited; holding right mouse chooses what is being rendered.

**Lens & exposure** controls iris (f-number), shutter angle, shutter time in milliseconds, shutter FPS, ISO, and
exposure compensation in stops. Angle and time are two views of the same setting: `time = angle / (360 × FPS)`.
For example, 180 degrees at 60 FPS gives 1/120 second. Shutter FPS is a camera setting and does not change the
render frame rate or frame cap. Exposure is relative to f/5.6, ISO 100 and 1/120 second; it affects splats and the
HDR sky. Scans are not calibrated radiance, so highlights can clip at bright settings.

**Focus & motion** sets focus distance in metres, sensor height, depth of field, and motion blur. Both blur effects
start disabled to keep existing gameplay sharp. Depth of field expands splats using a thin-lens circle-of-confusion
approximation, capped at 24 pixels. Motion blur expands each splat along the camera's projected movement over the
shutter interval, capped at 32 pixels and at one submitted frame. It resets after pauses, cuts and large camera
jumps. These are real-time Gaussian approximations: the sky and UI are not blurred, and moving objects do not yet
have independent motion vectors. Aperture and shutter still affect exposure while blur is disabled.

Focus, iris, ISO, shutter time and shutter FPS use logarithmic sliders for useful control over wide ranges. In the
browser, standard and zoom lens changes are saved separately between scene changes and reloads; resetting one lens
restores only its scene defaults. Camera pose is not saved as a lens preference. Runtime edits do not rewrite the scene JSON.

## ECS and Lua

`CameraLens` is a validated, renderer-independent ECS component. Add it to a camera in a scene manifest and register
`countersplat_scene_ecs` adapters as usual. The browser host inserts defaults for older scenes that omit it. The
player's component settings also appear in the entity inspector via `engine.export`; `CameraZoomLens` exposes
the same validated fields for the zoom profile. The camera panel's behavior and linked controls are implemented in
`assets/scripts/camera_lens.lua`, while `camera_zoom.lua` publishes the interpolated `camera.render_lens` resource.

```lua
local player = ecs.get_resource("scene.entities").player
local lens = ecs.get(player, "CameraLens")
lens.aperture_f_number = 2.8
lens.shutter_angle_degrees = 180
lens.shutter_fps = 60
lens.focus_distance_m = 3
lens.depth_of_field = true
ecs.set(player, "CameraLens", lens)
-- Optional: ask the browser host to persist this change locally.
ecs.emit("camera.lens_changed", lens)
local zoom = ecs.get(player, "CameraZoomLens")
zoom.focus_distance_m = 25
zoom.aperture_f_number = 8
ecs.set(player, "CameraZoomLens", zoom)
ecs.emit("camera.zoom_lens_changed", zoom)
```

`PlayerCamera.yaw`, `.pitch`, and `.roll` are radians; `.fov_degrees` is degrees. `view_fov_degrees` remains the
optional temporary zoom override. Other hosts can consume `CameraLens` and the renderer's generic
`GsplatScene::set_camera_effects` interface without depending on the browser or Lua.

The shutter relationship follows [ARRI's camera documentation](https://www.arri.com/resource/blob/403596/9bca057cb6a962cff63484fd092e4b33/alexa-35-sup-6-0-0-user-manual-en-data.pdf).
The thin-lens approximation is based on [NVIDIA's depth-of-field survey](https://developer.nvidia.com/gpugems/gpugems/part-iv-image-processing/chapter-23-depth-field-survey-techniques).

Camera exposure feeds the HDR scene before the [image-correction pipeline](post-processing.md);
its exposure control adds a separate creative adjustment in stops.
