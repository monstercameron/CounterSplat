-- Run in a host that registers countersplat-ui-ecs and publishes FrameMeter measurements.
-- The host owns the ECS world; Lua creates and customizes components through the bridge.
ui_frame_counter = ecs.spawn()
ecs.set(ui_frame_counter, "UiElement", {
    rect = { x = 20, y = 20, width = 230, height = 28 },
    anchor = "top_right",
    z = 100,
    widget = { type = "text", text = "", align = "left" },
    style = { font_size = 15 }
})
ecs.set(ui_frame_counter, "FrameCounter", { precision = 1 })

-- A host can run this system on its normal script schedule. Color changes make low
-- submitted-frame cadence visible; they are not a measure of GPU execution time.
ecs.system("ui.frame_counter", function(_dt)
    local counter = ecs.get(ui_frame_counter, "FrameCounter")
    local element = ecs.get(ui_frame_counter, "UiElement")
    if not counter or not element or not counter.measurement or counter.measurement == ecs.null then return end
    if counter.measurement.fps < 30 then
        element.style.text = { 255, 174, 146, 255 }
    else
        element.style.text = { 234, 245, 235, 255 }
    end
    ecs.set(ui_frame_counter, "UiElement", element)
end)
