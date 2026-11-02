# Transient scene lighting

Lua's reusable event API offers `engine.events.on(name, callback)`, `engine.events.emit(name, payload)`,
and `engine.events.off(name, callback)`. User-defined names share a single ECS transport channel; the
browser dispatches it after each gameplay tick and after each menu update. Delivery is queued, so events
emitted by a listener run on the next event pass. Native hosts should dispatch the `engine.event` ECS
channel during their event loop. A system can emit a short world-space light with:

```lua
local position, color = ecs.array(), ecs.array()
position[1], position[2], position[3] = 1, 2, 3
color[1], color[2], color[3] = 1, 0.68, 0.35
engine.events.emit('lighting.flash', {
    position = position, radius = 3, color = color,
    intensity = 0.32, duration_s = 0.09,
})
```

Position and radius use scene world units; color channels are linear RGB from 0 to 1.
`flash_lighting.lua` validates the event, publishes `render.local_light`, and applies a quadratic
fade. The browser acknowledges the first submitted frame so catch-up ticks cannot erase a pulse
before it is displayed. Other hosts should acknowledge `lighting.flash_started` by dispatching
`lighting.flash_presented` with its `id` after submitting the light. `input.reset` clears it. The current renderer has one bounded transient
light slot; the newest valid event replaces the previous one.

`muzzle_light.lua` is an adapter: it listens for `weapon.shot`, places a warm light just ahead of
the eye along the shot direction, and emits the reusable `lighting.flash` event. Its enabled state, intensity, radius,
duration, muzzle offset, and color channels are fields of the first-person entity's `MuzzleLight`
component and are exported to the ECS inspector. Scene scale converts meter settings to world units.

The splat shader applies smooth distance falloff in linear color. Scans contain baked lighting and
lack reliable surface normals, so this approximation does not cast shadows. The sky, HUD and
first-person video layer are unaffected. Lua owns event selection and pulse lifetime; the renderer
accepts only a validated light snapshot and has no weapon-specific behavior.
