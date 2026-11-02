-- The pause menu owns navigation and emits setting requests to the host.
-- The host validates renderer limits, persists preferences and replies with the
-- accepted state through menu.settings; no rendering policy lives in this UI.
local ids = assert(ecs.get_resource("scene.entities"), "menu UI entities are missing")
assert(ids.resume and ids.levels, "menu buttons are missing")
local display = {mode="fixed", width=1280, height=720, scaling="fill", aspect="16:9"}
local show_performance = true
local snapshot = {}
local page = "overview"
local presets = {
    {id="640x360", label="640 × 360", width=640, height=360},
    {id="1280x720", label="1280 × 720", width=1280, height=720},
    {id="1920x1080", label="1920 × 1080", width=1920, height=1080},
    {id="2560x1440", label="2560 × 1440", width=2560, height=1440},
    {id="3840x2160", label="3840 × 2160", width=3840, height=2160},
}
local aspect_buttons = { ["16:9"]=ids.aspect_16_9, ["16:10"]=ids.aspect_16_10,
    ["21:9"]=ids.aspect_21_9, custom=ids.aspect_custom }

local function widget(id, field, value)
    local element = id and ecs.get(id, "UiElement")
    if element and element.widget[field] ~= value then
        element.widget[field] = value
        ecs.set(id, "UiElement", element)
    end
end
local function highlight_aspect(id, selected)
    local element = id and ecs.get(id, "UiElement")
    if not element then return end
    local fill = selected and {120,87,49,240} or {15,23,28,220}
    local border = selected and {217,166,99,245} or {72,89,97,175}
    local changed = false
    for i=1,4 do
        if element.style.fill[i] ~= fill[i] then element.style.fill[i] = fill[i]; changed=true end
        if element.style.border[i] ~= border[i] then element.style.border[i] = border[i]; changed=true end
    end
    if changed then ecs.set(id,"UiElement",element) end
end
local groups={overview=ids.page_overview,settings=ids.page_settings,display=ids.page_display,
    controls=ids.page_controls,graphics=ids.page_graphics,audio=ids.page_audio,mode=ids.page_game_mode}
local navigation={settings=ids.nav_settings,display=ids.nav_display,controls=ids.nav_controls,
    graphics=ids.nav_graphics,audio=ids.nav_audio,mode=ids.nav_game_mode}
local function visibility(id, value)
    local element=id and ecs.get(id,"UiElement")
    if element and element.visible~=value then element.visible=value; ecs.set(id,"UiElement",element) end
end
local function set_page(name)
    if not groups[name] then return end
    page=name
    for key,id in pairs(groups) do visibility(id,key==page) end
    visibility(ids.back_overview,page~="overview")
    for key,id in pairs(navigation) do highlight_aspect(id,key==page) end
end
local function set_slider(id,value,minimum,maximum,enabled)
    local e=id and ecs.get(id,"UiElement")
    if not e then return end
    local changed=false
    for key,item in pairs({value=value,min=minimum,max=maximum}) do
        if item~=nil and e.widget[key]~=item then e.widget[key]=item;changed=true end
    end
    if enabled~=nil and e.enabled~=enabled then e.enabled=enabled;changed=true end
    if changed then ecs.set(id,"UiElement",e) end
end
local function set_checked(id,value,enabled)
    local e=id and ecs.get(id,"UiElement")
    if not e then return end
    local changed=e.widget.checked~=value
    e.widget.checked=value
    if enabled~=nil and e.enabled~=enabled then e.enabled=enabled;changed=true end
    if changed then ecs.set(id,"UiElement",e) end
end
local function key_name(code,default)
    if type(code)~="string" or code=="" then return default end
    local names={ShiftLeft="Shift",ShiftRight="Shift",ControlLeft="Ctrl",ControlRight="Ctrl",
        Space="Space",ArrowUp="↑",ArrowDown="↓",ArrowLeft="←",ArrowRight="→"}
    return names[code] or code:gsub("^Key",""):gsub("^Digit","")
end
local function grouped(value)
    if type(value)~="number" or value~=value then return "—" end
    local reversed=tostring(math.floor(math.max(0,value))):reverse()
    return reversed:gsub("(%d%d%d)","%1,"):reverse():gsub("^,","")
end
local function playtime(seconds)
    if type(seconds)~="number" or seconds~=seconds then return "—" end
    local s=math.floor(math.max(0,seconds))
    if s>=3600 then return string.format("%d:%02d:%02d",math.floor(s/3600),math.floor(s/60)%60,s%60) end
    return string.format("%d:%02d",math.floor(s/60),s%60)
end
local function ratio_label(width,height)
    local a,b=math.floor(width),math.floor(height)
    while b~=0 do a,b=b,a%b end
    local result = tostring(math.floor(width/a))..":"..tostring(math.floor(height/a))
    return #result <= 9 and result or nil
end
local function current_index()
    for index, preset in ipairs(presets) do
        if (display.mode == "viewport" and preset.id == "viewport") or
           (display.mode == "fixed" and preset.width == display.width and preset.height == display.height) then
            return index
        end
    end
    return nil
end
local function sync(message)
    local index = current_index()
    local title = index and presets[index].label or (tostring(display.actual_width or display.width).." × "..tostring(display.actual_height or display.height))
    widget(ids.resolution_current, "text", title)
    highlight_aspect(ids.resolution_native, display.mode == "viewport")
    for aspect,id in pairs(aspect_buttons) do highlight_aspect(id, display.aspect == aspect) end
    local ratio=ratio_label(display.ratio_width or display.actual_width or display.width,display.ratio_height or display.actual_height or display.height)
    widget(ids.aspect_custom, "label", display.aspect == "custom" and ratio and ("Custom · "..ratio) or "Custom")
    widget(ids.resolution_width, "text", tostring(display.width))
    widget(ids.resolution_height, "text", tostring(display.height))
    local scaling_labels={fill="Fill · crop edges", fit="Fit · show bars", pixel="Pixel perfect · integer"}
    widget(ids.scaling_toggle, "label", scaling_labels[display.scaling] or scaling_labels.fill)
    widget(ids.show_performance, "checked", show_performance)
    widget(ids.feedback, "text", message or "")
    local game=type(snapshot.game)=="table" and snapshot.game or {}
    local controls=type(snapshot.controls)=="table" and snapshot.controls or {}
    local bindings=type(controls.bindings)=="table" and controls.bindings or {}
    local graphics=type(snapshot.graphics)=="table" and snapshot.graphics or {}
    local audio=type(snapshot.audio)=="table" and snapshot.audio or {}
    local level_lod=type(graphics.lod)=="number" and tostring(math.floor(graphics.lod)) or "Auto"
    widget(ids.status_level_name,"text",type(game.name)=="string" and game.name or "Current level")
    widget(ids.status_level_kind,"text",game.format=="streamed_sog" and "Streamed scan" or game.format=="single_sog" and "Single scan" or "Exploration")
    widget(ids.status_value_splats,"text",grouped(game.resident_splats))
    widget(ids.status_value_lod,"text",level_lod)
    local chunks=type(game.chunks_loaded)=="number" and grouped(game.chunks_loaded) or "—"
    widget(ids.status_value_chunks,"text",chunks)
    widget(ids.status_value_playtime,"text",playtime(game.playtime_seconds))
    local aspect=display.aspect and display.aspect~="custom" and (" ("..display.aspect..")") or ""
    widget(ids.overview_display_value_resolution,"text",title..aspect)
    widget(ids.overview_display_value_scaling,"text",scaling_labels[display.scaling] or scaling_labels.fill)
    widget(ids.overview_display_value_sky,"text",graphics.sky_enabled==true and "On" or graphics.sky_enabled==false and "Off" or "—")
    local shortcuts={
        {table.concat({key_name(bindings.forward,"W"),key_name(bindings.left,"A"),key_name(bindings.backward,"S"),key_name(bindings.right,"D")},"  "),"Move"},
        {"Mouse","Look"},{"Q","Inspect HUD"},{key_name(bindings.jump,"Space"),"Jump"},
        {key_name(bindings.sprint,"Shift"),"Sprint"},{key_name(bindings.crouch,"Ctrl"),"Crouch"},{"R","Reload"},{"V","Fire mode"},
    }
    for i,item in ipairs(shortcuts) do
        widget(ids["overview_key_"..i],"text",item[1]); widget(ids["overview_action_"..i],"text",item[2])
        widget(ids["controls_key_"..i],"text",item[1]); widget(ids["controls_action_"..i],"text",item[2])
    end
    if type(controls.move_speed)=="number" then set_slider(ids.controls_speed,controls.move_speed) end
    if type(controls.mouse_sensitivity)=="number" then set_slider(ids.controls_sensitivity,controls.mouse_sensitivity) end
    if type(controls.invert_y)=="boolean" then set_checked(ids.controls_invert,controls.invert_y) end
    if type(graphics.draw_distance)=="number" then set_slider(ids.graphics_distance,graphics.draw_distance) end
    if type(graphics.sky_enabled)=="boolean" then set_checked(ids.graphics_sky,graphics.sky_enabled) end
    local lod_count=type(graphics.lod_count)=="number" and math.max(1,math.floor(graphics.lod_count)) or 1
    local has_lods=lod_count>1
    set_checked(ids.graphics_auto,type(graphics.lod)~="number",has_lods)
    set_slider(ids.graphics_lod,type(graphics.lod)=="number" and graphics.lod or 0,0,math.max(1,lod_count-1),has_lods)
    widget(ids.graphics_hint,"text",has_lods and "0 is the finest available level of detail." or "This level has one level of detail.")
    if type(audio.gain)=="number" then set_slider(ids.audio_gain,audio.gain) end
    if type(audio.muted)=="boolean" then set_checked(ids.audio_mute,audio.muted) end
    local mode=type(snapshot.mode_state)=="table" and snapshot.mode_state or {}
    local cfg=type(snapshot.mode_config)=="table" and snapshot.mode_config or {}
    local gun=type(snapshot.weapon_ballistics)=="table" and snapshot.weapon_ballistics or {}
    local status=type(mode.status)=="string" and mode.status or "ready"
    local status_labels={ready="READY",running="IN PROGRESS",defeated="DEFEATED"}
    widget(ids.mode_status,"text","STATUS  ·  "..(status_labels[status] or "READY"))
    widget(ids.mode_stats,"text",string.format("KILLS  %s      SHOTS  %s      HITS  %s",grouped(mode.kills or 0),grouped(mode.shots or 0),grouped(mode.hits or 0)))
    widget(ids.mode_rules,"text",string.format("TARGET %s HP    ·    HITBOX %s HP    ·    SHOT %s DMG",
        grouped(cfg.target_health or 100),grouped(cfg.zone_health or 100),grouped(gun.damage or 35)))
    widget(ids.mode_start,"label",status=="running" and "Restart pursuit" or status=="defeated" and "Play again" or "Start pursuit")
end
local function apply_state(value)
    if type(value) ~= "table" then return end
    snapshot=value
    if type(value.display) == "table" then display = value.display end
    if type(value.show_performance) == "boolean" then show_performance = value.show_performance end
    if type(value.presets) == "table" and #value.presets > 0 then presets = value.presets end
    sync(value.error)
end
ecs.on("menu.settings", apply_state)
-- Scale the authored layout uniformly so it remains inside small viewports.
local original = {}
for name,id in pairs(ids) do
    if name~="backdrop" then
        local e=ecs.get(id,"UiElement")
        if e then original[id]={x=e.rect.x,y=e.rect.y,width=e.rect.width,height=e.rect.height,font_size=e.style.font_size,
            points=e.widget.type=="polyline" and e.widget.points or nil} end
    end
end
ecs.on("menu.viewport", function(viewport)
    if type(viewport) ~= "table" or type(viewport.width) ~= "number" or type(viewport.height) ~= "number" then return end
    local scale = math.max(.1, math.min(1, (viewport.width - 24) / 1120, (viewport.height - 24) / 700))
    for id, base in pairs(original) do
        local e = ecs.get(id, "UiElement")
        if e then
            e.rect.x, e.rect.y = base.x * scale, base.y * scale
            e.rect.width, e.rect.height = base.width * scale, base.height * scale
            e.style.font_size = math.max(1, base.font_size * scale)
            if e.widget.type=="polyline" then
                local points=ecs.array()
                -- Icons are authored at 24 units and scale with the layout.
                local source=base.points
                if source then
                    for i,point in ipairs(source) do local q=ecs.array();q[1],q[2]=point[1]*scale,point[2]*scale;points[i]=q end
                    e.widget.points=points
                end
            end
            ecs.set(id, "UiElement", e)
        end
    end
end)
ecs.on("menu.opened",function() set_page("overview") end)
local function request(next_display)
    ecs.emit("settings.display_request", next_display)
end
local function choose(direction)
    if #presets == 0 then return end
    local current = current_index()
    local chosen
    if current then
        -- Do not wrap from the smallest setting to the largest (or vice versa).
        chosen = presets[current + direction]
    else
        -- Native viewport size can sit between presets. Step to the nearest
        -- size in the requested direction instead of jumping to an extreme.
        local pixels = (display.actual_width or display.width) * (display.actual_height or display.height)
        if direction > 0 then
            for _, preset in ipairs(presets) do
                if preset.width * preset.height > pixels then chosen = preset; break end
            end
        else
            for i = #presets, 1, -1 do
                if presets[i].width * presets[i].height < pixels then chosen = presets[i]; break end
            end
        end
    end
    if not chosen then return end
    if chosen.id == "viewport" then
        request({mode="viewport", width=display.width, height=display.height, scaling=display.scaling, aspect=display.aspect})
    else
        request({mode="fixed", width=chosen.width, height=chosen.height, scaling=display.scaling, aspect=display.aspect,
            ratio_width=display.aspect == "custom" and display.ratio_width or nil,
            ratio_height=display.aspect == "custom" and display.ratio_height or nil})
    end
end
local function custom()
    local width = ecs.get(ids.resolution_width, "UiElement").widget.text
    local height = ecs.get(ids.resolution_height, "UiElement").widget.text
    if not width:match("^%d+$") or not height:match("^%d+$") then
        widget(ids.feedback, "text", "Enter whole-number width and height.")
        return
    end
    request({mode="fixed", width=tonumber(width), height=tonumber(height), scaling=display.scaling, aspect=display.aspect, reset_ratio=display.aspect == "custom" and true or nil})
end
set_page("overview")
sync()
ecs.system("menu.navigation", function(_dt)
    local keys = ecs.drain("input.key")
    local actions = ecs.drain("ui.action")
    for _, key in ipairs(keys) do
        if key.key == "Escape" and key.pressed and not key.is_repeat then
            engine.scene.pop()
            return
        end
    end
    for _, action in ipairs(actions) do
        local id, kind = action.id, action.kind
        if kind.type == "clicked" then
            if id == ids.resume then engine.scene.pop(); return
            elseif id == ids.levels or id == ids.return_menu then engine.scene.replace("level_selector"); return
            elseif id == ids.back_overview then set_page("overview")
            elseif id == ids.nav_settings then set_page("settings")
            elseif id == ids.nav_display then set_page("display")
            elseif id == ids.nav_controls then set_page("controls")
            elseif id == ids.nav_graphics then set_page("graphics")
            elseif id == ids.nav_audio then set_page("audio")
            elseif id == ids.nav_game_mode then set_page("mode")
            elseif id == ids.mode_start then
                ecs.emit("game.mode_request",{mode="pursuit",action="start"})
                engine.scene.pop()
                return
            elseif id == ids.mode_reset then
                ecs.emit("game.mode_request",{mode="pursuit",action="reset"})
            elseif id == ids.aspect_16_9 then ecs.emit("settings.aspect_request", {aspect="16:9"})
            elseif id == ids.aspect_16_10 then ecs.emit("settings.aspect_request", {aspect="16:10"})
            elseif id == ids.aspect_21_9 then ecs.emit("settings.aspect_request", {aspect="21:9"})
            elseif id == ids.aspect_custom then ecs.emit("settings.aspect_request", {aspect="custom"})
            elseif id == ids.resolution_native then
                request({mode="viewport", width=display.width, height=display.height, scaling=display.scaling, aspect=display.aspect})
            elseif id == ids.resolution_previous then choose(-1)
            elseif id == ids.resolution_next then choose(1)
            elseif id == ids.resolution_apply then custom()
            elseif id == ids.scaling_toggle then
                local next_scaling = display.scaling == "fill" and "fit" or display.scaling == "fit" and "pixel" or "fill"
                request({mode=display.mode, width=display.width, height=display.height,
                    scaling=next_scaling, aspect=display.aspect})
            end
        elseif id == ids.show_performance and kind.type == "bool_changed" then
            ecs.emit("settings.hud_request", {show_performance=kind.value == true})
        else
            local targets={[ids.controls_speed]="controls_speed",[ids.controls_sensitivity]="controls_sensitivity",
                [ids.controls_invert]="controls_invert",[ids.graphics_distance]="level_draw_distance",
                [ids.graphics_auto]="level_auto",[ids.graphics_lod]="level_lod",[ids.graphics_sky]="sky_enabled",
                [ids.audio_gain]="audio_gain",[ids.audio_mute]="audio_muted"}
            if targets[id] and (kind.type=="number_changed" or kind.type=="bool_changed") then
                ecs.emit("settings.gameplay_request",{target=targets[id],kind=kind.type,value=kind.value})
            end
        end
    end
end)
