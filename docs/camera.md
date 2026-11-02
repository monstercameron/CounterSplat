# Camera and lens

Open **Camera & lens** in the gameplay tool stack. The panel can be collapsed or dragged like the
other ECS panels. **View** changes the player's vertical field of view, yaw, pitch, and roll; mouse look and the
existing right-button optical zoom continue to work. **Reset view** restores the spawn pose, while **Reset lens**
restores the scene's camera-lens defaults.

**Standard lens** and **zoom lens** retain independent iris, shutter, ISO, exposure, focus, sensor,
depth-of-field, and motion-blur settings. Select **Edit zoom lens** to tune the ADS profile. The
camera panel's **Optic** button cycles the equipped weapon between iron sights, red dot, and scope.
Iron sights use 1.25×; red dots use 1.5×. A scope uses its own `WeaponOptic.scope_magnification`,
which accepts values from 1× to 100× and can be edited with the scope magnification slider while editing the zoom lens or via the
first-person entity inspector. Resetting a camera lens does not change the equipped optic.

Holding right mouse or emitting `weapon.aim` smoothly blends the photographic settings and the
weapon's optical magnification. The projection uses the tangent of half the base FOV, so the
specified multiplier remains correct at different base fields of view. The camera returns to its
standard FOV on release, pause, or input reset and does not zoom with a stowed weapon.
The current first-person footage depicts iron sights; red-dot and scope selection changes camera
magnification only until matching attachment visuals are supplied.

**Lens & exposure** controls iris (f-number), shutter angle, shutter time in milliseconds, shutter FPS, ISO, and
exposure compensation in stops. Angle and time are two views of the same setting: `time = angle / (360 × FPS)`.
The gameplay preset uses 180 degrees at a 24 FPS camera reference (1/48 second), a common cinema exposure.
Shutter FPS is a camera setting and does not change the render frame rate or frame cap. Exposure is relative to f/5.6, ISO 100 and 1/120 second; it affects splats and the
HDR sky. Scans are not calibrated radiance, so highlights can clip at bright settings.

**Focus & motion** sets focus distance in metres, sensor height, depth of field, and motion blur. The gameplay
scene enables subtle depth of field on both lenses, with a 7.5 m standard focus and a 15 m zoom focus. Motion
blur starts enabled for both profiles. At the 1/48-second shutter, the standard lens uses f/8 at ISO 80 and the
zoom lens f/16 at ISO 320, matching the previous scene brightness. Depth of field expands splats using a thin-lens
circle-of-confusion approximation, capped at 24 pixels. Motion blur expands each splat along the camera's projected
movement over the shutter interval. The previous submitted displacement is scaled by exposure time, allowing a
shutter longer than one display frame, with a maximum of eight frame intervals and a 32-pixel footprint. The
current projection is used for both positions, so optical zoom alone does not create radial motion streaks. Motion
history resets after pauses, cuts and large camera jumps. These are real-time Gaussian approximations: the sky,
first-person layer and UI are not blurred, and moving objects do not yet have independent motion vectors. Aperture
and shutter still affect exposure while blur is disabled.

Focus, iris, ISO, shutter time and shutter FPS use logarithmic sliders for useful control over wide ranges. In the
browser, standard and zoom lens changes are saved separately between scene changes and reloads; resetting one lens
restores only its scene defaults. Camera pose is not saved as a lens preference. Runtime edits do not rewrite the scene JSON.

## ECS and Lua

`CameraLens` is a validated, renderer-independent ECS component. Add it to a camera in a scene manifest and register
`countersplat_scene_ecs` adapters as usual. The browser host inserts defaults for older scenes that omit it. The
player's component settings also appear in the entity inspector via `engine.export`; `CameraZoomLens` exposes
the same validated fields for the zoom profile. The camera panel's behavior and linked controls are implemented in
`assets/scripts/camera_lens.lua`, while `camera_zoom.lua` publishes the interpolated `camera.render_lens` resource.
`WeaponOptic` is a weapon-owned dynamic ECS component; `weapon_optics.lua` exposes `engine.optics.set`
and `engine.optics.magnification` so an attachment system can select the optic without camera-specific code.
The browser persists the first-person weapon's selected optic and scope power separately from camera lens settings.

```lua
local weapon = ecs.get_resource("scene.entities").first_person
engine.optics.set(weapon, {kind="red_dot"}) -- 1.5×
engine.optics.set(weapon, {kind="scope", scope_magnification=6}) -- 6×
engine.optics.set(weapon, {kind="iron"}) -- 1.25×
-- Alternatively, emit weapon.optic_set with the same fields and optional entity.
```

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
