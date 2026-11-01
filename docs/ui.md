# UI layer

CounterSplat's UI is made of ordinary ECS entities. It draws into the same wgpu color target as the scene in a
separate load pass. The example browser host draws both the scan and UI directly into its WebGPU canvas.

## Crate boundaries

- `countersplat-ui` contains serializable `UiElement` and `FrameCounter` components, geometry, colors, styles,
  input/events, snapshots, and the frame-cadence meter. It knows nothing about ECS, Lua, browsers, or the GPU.
- `countersplat-ui-ecs` registers validated component adapters, resolves entity hierarchy into a `UiFrame`, and
  applies control events on the world owner. This adapter knows nothing about the renderer or Lua implementation.
- `countersplat-ui-wgpu` renders a `UiFrame` using egui/wgpu and returns owned `UiEvent`s. Its font atlas and input
  context are owned by the renderer; it has no ECS dependency.

Built-in widgets include panels, rectangles, circles, lines, text, buttons, checkboxes, sliders, single-line text
input, progress bars, bounded line graphs, and vertical scroll areas. Each element has an anchor, local rectangle in logical points, global z value, visibility,
enabled state, clipping, and style. A UI parent's rectangle establishes the child's coordinate space; right/bottom
anchor offsets are insets. Layout inherits the parent's visibility, enabled state, and clipping. Elements with the
same z draw in stable ID order. Provide colors and fonts through `Style`; `Font::Proportional` and `Font::Monospace`
are bundled. Register a custom TTF/OTF with `UiRenderer::add_font` and use `Font::Named` with the same name; an
unknown name falls back to proportional. Supply valid font bytes from a trusted asset source.

## Collapsible panels

A panel can have an optional `collapsible` configuration. It owns its title, header height, and current
`collapsed` state as part of the `UiElement` component. Clicking the header emits a `collapsed_changed` event
and the ECS adapter applies the new state; dragging the header moves the panel and its descendants. A collapsed panel draws only its header bar;
all descendants are hidden and cannot accept stale input. Their components and individual visibility settings are
preserved, including nested panel states. Expanding restores the same children and their previous values.

Add this to a panel's `UiElement` in `assets/scenes/gameplay.json`:

```json
{
  "rect": { "x": 20, "y": 20, "width": 240, "height": 180 },
  "widget": { "type": "panel" },
  "collapsible": { "title": "Settings", "header_height": 32, "collapsed": false }
}
```

Child coordinates are relative to the content area **below** the header. The stored rectangle is always the expanded
size; collapse changes only the extracted drawing rectangle. Header height must be finite, between 18 and 128 points,
and smaller than the expanded panel height. Only `panel` widgets can use this option. Panels without `collapsible`
retain their original layout. Top, bottom, and center anchors still position the current visible rectangle normally.
The browser's Scene view, Performance, and HDR sky panels use this configuration. The performance history keeps
recording while its panel is collapsed. Runtime changes are not written back to `assets/scenes/gameplay.json` automatically.

From Lua, read or update the same component and use the normal validated ECS bridge:

```lua
local element = ecs.get(panel, "UiElement")
element.collapsible.collapsed = true
ecs.set(panel, "UiElement", element)
-- A clicked header also produces { id = panel, kind = { type = "collapsed_changed", value = true } }
-- on the ui.action event queue.
```

## Scroll areas

A `scroll_area` is a clipped viewport with a vertical scrollbar. The browser
routes wheel and trackpad input to the nearest visible scroll area under the
pointer; its scrollbar can also be dragged. Child coordinates are relative to
the full content, and the renderer moves them by the ECS-owned offset.

```json
{
  "rect": { "x": 12, "y": 32, "width": 236, "height": 350 },
  "widget": { "type": "scroll_area", "offset": 0, "content_height": 900 }
}
```

Offsets and content height are logical points. The offset must be between zero
and `max(0, content_height - rect.height)`. Accepted wheel and scrollbar
changes update the `UiElement` and emit `ui.action` with
`{ type = "scroll_changed", value = offset }`. Scripts can change the same
validated fields through `ecs.get` and `ecs.set`; when shrinking content,
clamp the offset at the same time. Scroll areas always clip their descendants.

The ECS entity browser reuses a pool of rows positioned in a scroll area and
grows that pool as the panel expands. Selecting an entity opens its settings
without resetting the scroll position. A moved
collapsible panel containing a scroll area is kept fully in view when it fits
within its parent, so its last rows remain reachable after a resize.

## Resizing and entity ownership

Drag the lower-right grip of **ECS entities** to resize the panel. Its list and
scrollbar follow the new dimensions; the browser saves the size with its local
UI layout and **Reset positions** restores the scene default. Existing saved
layouts containing only positions continue to work.

The + and - controls expand and collapse actual ECS child relationships.
Select an entity, choose **Set parent**, then select its intended owner in the
tree. **Cancel** leaves it unchanged; **Make root** clears its parent. The ECS
rejects cycles, and the panel protects its own editor controls from reparenting.
Relationships changed in the panel last for the current scene session; edit the
scene JSON to make them part of the authored scene. UI entity parenting also
affects UI layout, visibility and clipping.

The gameplay JSON groups the camera controller and first-person weapon under the
player, skybox and image correction under scene visuals, and UI panels under
HUD, player, scene, and editor groups. These non-UI grouping entities organize
ownership without becoming UI coordinate spaces. The level remains its own
root. Parent links stay within their scene layers so layers can still be loaded
independently. Set `preview.entity_browser_expanded` to scene entity IDs to
choose which branches start open. Non-UI parent links do not automatically
apply transforms; ordinary despawn detaches surviving children, while recursive
despawn explicitly removes the full subtree.

A panel opts in with `resize` limits in logical points, for example:

```json
"resize": { "min": [250, 200], "max": [700, 1000] }
```

The renderer emits `resized_to` with the requested width and height; the
UI ECS adapter validates and applies it. Scripts receive the same `ui.action`
event and can resize dependent controls. For hierarchy-aware scripts,
`ecs.hierarchy()` returns a compact list of `{id, parent}` entries without
copying component or resource payloads. A root's parent is `ecs.null`.

## Moving canvas UI

Drag an expanded or collapsed panel by its header. In the browser preview, select **Arrange UI** to drag individual
controls, shapes, text, or panels; select **Done arranging** to use controls normally again. While arranging,
**Reset positions** restores the current scene's authored positions. The selector and gameplay keep separate
browser-local layouts across refreshes and scene changes; scene JSON is not rewritten. Storage is local to that
browser and origin, and positions fall back to scene defaults if local storage is unavailable.

Each `UiElement` has `draggable` (default `true`) and `drag_offset` (default `[0, 0]`) properties. Set
`"draggable": false` in a scene component to lock an element. Offsets are logical screen points applied after
anchoring; moving a parent also moves its children, and clipping and enabled/visible state still apply. A moved
element remains reachable when its viewport or clipping parent shrinks. The browser's selector and gameplay
persist authored scene entities and generated level rows by stable ID.

Scripts can read or set the offset through the existing ECS bridge. Accepted drags also produce `ui.action`
events whose `kind` is `{ type = "moved_by", value = { dx, dy } }`; the values are incremental screen-point
offsets applied to the component, including any correction required after a resize.

```lua
local element = ecs.get(panel, "UiElement")
element.drag_offset = { 24, 12 }
ecs.set(panel, "UiElement", element)
-- Or lock it against pointer dragging:
element.draggable = false
ecs.set(panel, "UiElement", element)
```

A native host can call `UiRenderer::set_arrange_mode(true)` and route returned events to `apply_events`; it owns
its own choice of persistence. The browser preview implements persistence with local storage.

## ECS and Lua

Register the adapters once on the world owner with `countersplat_ui_ecs::register(&mut world)`. The generic Lua ECS
bridge can then create, read, update, query, and remove `UiElement` and `FrameCounter` components. For example:

```lua
local counter = ecs.spawn()
ecs.set(counter, "UiElement", {
  rect = { x = 20, y = 20, width = 220, height = 28 },
  anchor = "top_right",
  z = 100,
  widget = { type = "text", text = "", align = "left" }
})
ecs.set(counter, "FrameCounter", { precision = 1 })

-- Later, after the host publishes a measurement:
local frame = ecs.get(counter, "FrameCounter")
if frame.measurement and frame.measurement ~= ecs.null then
  local fps = frame.measurement.fps
  local frame_ms = frame.measurement.frame_time_ms
  -- Use these values to update any other UI entity or game state.
end
frame.show_frame_time = false  -- Optional; show_fps and precision are also configurable.
ecs.set(counter, "FrameCounter", frame)
```

A `FrameCounter` attached beside a text `UiElement` supplies the text in the extracted snapshot; the underlying
`UiElement` text is not rewritten. The script can style, move, hide, parent, or remove that entity normally.
Precision is 0–3 decimal places. Invalid script writes are rejected without replacing the old component. Hosts
publish measurements to counters with `update_frame_counters`; a script can read `measurement.intervals` as well
as FPS and frame time. `measurement` is `ecs.null` while warming up or after a reset.

For interactive controls, read or write `UiElement` through `ecs.get`/`ecs.set`. Accepted panel, button, checkbox, slider,
and text input events are copied to the destructive single-consumer `ui.action` ECS event queue. Scripts can read
it with `ecs.drain("ui.action")`. Each entry has the entity `id` and a `kind` object, for example
`{ id = ..., kind = { type = "bool_changed", value = false } }`. If several systems need the same events, the host
must distribute copies; draining is not a broadcast. `ecs.on` handles host-dispatched events, which are separate.

The runnable script `apps/threaded-demo/examples/fps_counter.lua` creates a counter and registers a Lua system
that changes its color based on measured FPS. Run it in a host that registers these adapters, publishes timing, and
schedules the Lua system. The native integration test in `apps/threaded-demo/tests/ui_lua.rs` runs that script
against the real adapters and verifies
component creation, validation, frame data, custom script formatting, and control events. The browser demo now hosts
Lua 5.4 for its gameplay and menu worlds; its menu and player scripts use these same ECS events and component APIs.
The gameplay scene, level, player and UI layers share a world and are exposed through that script bridge.
The **ECS entities** panel lists these live entities; selecting a row opens **Entity settings** on the right.
The inspector shows component names and edits physics or Lua-exported values; see [entity physics](entity-physics.md).

A slider widget can set `"logarithmic": true` to spread a wide range while preserving its actual numeric value.
The field defaults to false for older scene files and scripts. The camera panel uses it for values such as focus
distance and ISO.

## Frame timing semantics

Call `FrameMeter::record(monotonic_time)` once after each frame submission, and pass each `FrameUpdate::Measured`
to `update_frame_counters`. The meter reports the average interval and reciprocal FPS over roughly half-second
windows; it ignores repeated timestamps and resets on a backwards clock or a gap longer than two seconds. Clear
counter measurements on `FrameUpdate::Reset` and when pausing. These are **submitted-frame cadence** measurements,
not exact monitor-present times, CPU work duration, or GPU execution duration. GPU timestamps or presentation
feedback would require separate host instrumentation.

`FrameHistory` keeps a rolling 60-second graph history, with at most 241 quarter-second buckets. Each bucket stores
the slowest submitted-frame interval observed in that period, preserving short spikes. Plot `points(now)` with x
from 0 to 60 seconds and y in milliseconds using `Widget::LineGraph`. The browser example adapts its vertical
scale to the largest retained spike and labels the scale. It clears the history on pause, a backwards clock, or an
interruption longer than the entire 60-second window; shorter stalls remain visible.

## Host integration

1. Create or update entities on the world owner; use `spawn` for validated Rust-side creation and optional parenting.
2. Extract an owned `UiFrame` with the logical viewport rectangle. It can be sent to a render worker without sharing
   the world. Forward platform pointer, keyboard, text, focus, and scroll events to `UiRenderer::push_input` in
   logical points. Pointer moves coalesce and the input queue is bounded.
3. After rendering the scene, call `UiRenderer::draw` with a `UiTarget` containing the existing color view, target size, pixel scale, and
   monotonic time. Submit any returned callback command buffers before the main command encoder, then call
   `after_submit` so unused atlas textures can be released.
4. Send returned `UiEvent`s to the world owner and call `apply_events`. It ignores stale, foreign, disabled,
   hidden, invalid, or mismatched controls. Handle returned actions or route the `ui.action` queue to a script.

The browser forwards basic mouse, touch, key, and text input, including composition completion. The canvas itself is
focusable; a full browser accessibility tree for individual rendered widgets, comprehensive IME support, and
clipboard integration are not yet implemented. Hosts should route game controls around UI focus and pointer usage.
