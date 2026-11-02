-- Contextual inspection HUD. All timing, data selection, and layout live in Lua;
-- UiElement entities remain ordinary, noninteractive canvas UI.
local ids=assert(ecs.get_resource('scene.entities'))
local player, weapon, root=assert(ids.player),assert(ids.first_person),assert(ids.inspection_hud)
local ammo_root=assert(ids.ammo_hud)
local read_hud=engine.component_reader(root,'InspectionHud')
local read_weapon=engine.component_reader(weapon,'WeaponState')
local read_health=engine.component_reader(player,'Health')
local read_armor=engine.component_reader(player,'Armor')
local read_camera=engine.component_reader(player,'PlayerCamera')
local read_controls=engine.component_reader(player,'PlayerControls')
local read_rig=engine.component_reader(player,'PlayerRig')
local parts={
    'inspect_card','inspect_accent','inspect_title','inspect_rounds','inspect_reserve','inspect_mode',
    'inspect_rule','inspect_health_label','inspect_health_value','inspect_armor_label','inspect_armor_value',
    'inspect_health_track','inspect_health_fill','inspect_armor_track','inspect_armor_fill',
    'inspect_leader','inspect_target_ring','inspect_target_dot',
    'inspect_compass_ring','inspect_compass_north','inspect_compass_arrow','inspect_minimap','inspect_map_status',
    'inspect_objective_icon','inspect_objective_title','inspect_objective_detail',
    'inspect_health_icon_h','inspect_health_icon_v','inspect_armor_icon',
    'inspect_stamina_icon','inspect_stamina_value','inspect_stamina_track','inspect_stamina_fill',
    'inspect_speed_icon','inspect_speed_value','inspect_speed_track','inspect_speed_fill','inspect_weapon_icon',
}
for i=1,4 do
    for _,suffix in ipairs({'box','number','name'}) do parts[#parts+1]='inspect_slot'..i..'_'..suffix end
end
local base={}
for _,name in ipairs(parts) do
    local entity=assert(ids[name],name..' is missing')
    local element=assert(ecs.get(entity,'UiElement'))
    base[name]={fill=element.style.fill[4],text=element.style.text[4],border=element.style.border[4],
        font_size=element.style.font_size}
end
for _,item in ipairs({
    {'inspect_enabled','enabled','Inspection HUD'},
    {'inspect_duration','duration_s','Inspect duration (s)',.5,10},
    {'inspect_fade_in','fade_in_s','Inspect fade in (s)',.01,1},
    {'inspect_fade_out','fade_out_s','Inspect fade out (s)',.01,1},
    {'ammo_fire_seconds','ammo_fire_seconds','Ammo display after firing (s)',.1,5},
    {'inspect_map_enabled','map_enabled','Inspection map'},
    {'inspect_map_radius','map_radius_m','Map radius (m)',4,30},
    {'inspect_map_height','map_height_m','Map slice above floor (m)',.1,2},
    {'inspect_map_thickness','map_thickness_m','Map slice thickness (m)',.05,.8},
}) do
    local spec={component='InspectionHud',field=item[2],label=item[3]}
    if item[4] then spec.min,spec.max=item[4],item[5] end
    engine.export(root,item[1],spec)
end
local annotations=ecs.get_resource('first_person_magazine') or {}
local active,age=false,0
local presentation_seen,presented_pose,current_alpha=false,nil,0
local last_position,speed_mps,layout_elapsed=nil,0,0
local combat_held={}
local ammo_visible,ammo_remaining=false,0
local last_ammo_label_x,last_ammo_label_y=nil,nil
local map_info,map_status,map_last,map_elapsed=nil,"idle",nil,0
local function finite(v) return type(v)=='number' and v==v and math.abs(v)<=10000000 end
local function num(v,fallback,lo,hi)
    if not finite(v) then return fallback end
    return math.max(lo,math.min(hi,v))
end
local function clamp(v,lo,hi) return math.max(lo,math.min(hi,v)) end
local function set_root(visible,w,h)
    local e=assert(ecs.get(root,'UiElement'))
    local changed=e.visible~=visible
    e.visible=visible
    if w and (math.abs(e.rect.width-w)>.01 or math.abs(e.rect.height-h)>.01) then
        e.rect.width,e.rect.height=w,h; changed=true
    end
    if changed then ecs.set(root,'UiElement',e) end
end
-- The magazine callout has its own persistent layer, independent of Q stats.
local function set_ammo_bounds(w,h)
    local e=assert(ecs.get(ammo_root,'UiElement'))
    if math.abs(e.rect.width-w)>.01 or math.abs(e.rect.height-h)>.01 then
        e.rect.width,e.rect.height=w,h
        ecs.set(ammo_root,'UiElement',e)
    end
end
-- Keep the count up throughout either physical or scripted firing.
local function firing_held()
    return combat_held.mouse_left==true or combat_held.trigger==true
end
-- Ammo is separate from Q stats; request compositor tracking only while needed.
local function sync_ammo_visibility()
    local state=read_weapon() or {}
    local armed=state.state~='noGun' and state.state~='stow' and state.state~='equip' and
        state.state~='death' and state.state~='dead'
    local aiming=combat_held.mouse_right==true or combat_held.aim==true
    local wanted=active or (armed and (aiming or firing_held() or ammo_remaining>0))
    if wanted==ammo_visible then return end
    ammo_visible=wanted
    local element=assert(ecs.get(ammo_root,'UiElement'))
    element.visible=wanted
    ecs.set(ammo_root,'UiElement',element)
    if not wanted then
        presentation_seen,presented_pose=false,nil
        last_ammo_label_x,last_ammo_label_y=nil,nil
    end
    ecs.emit('hud.pose_tracking',{enabled=wanted})
end
local function show_firing_ammo()
    local cfg=read_hud() or {}
    ammo_remaining=num(cfg.ammo_fire_seconds,1,.1,5)
    sync_ammo_visibility()
end
local function set_part(name,rect,alpha,text,visible,points)
    local id=ids[name]; local e=assert(ecs.get(id,'UiElement')); local b=base[name]
    if visible==false then
        if e.visible then e.visible=false; ecs.set(id,'UiElement',e) end
        return
    end
    local changed=false
    if rect then
        for key,value in pairs(rect) do
            if math.abs(e.rect[key]-value)>.05 then e.rect[key]=value; changed=true end
        end
    end
    local show=visible~=false
    if e.visible~=show then e.visible=show; changed=true end
    for _,field in ipairs({'fill','text','border'}) do
        local value=math.floor(b[field]*alpha+.5)
        if e.style[field][4]~=value then e.style[field][4]=value; changed=true end
    end
    local size=math.max(1,b.font_size*(rect and rect.scale or 1))
    if math.abs(e.style.font_size-size)>.05 then e.style.font_size=size; changed=true end
    if text and e.widget.text~=text then e.widget.text=text; changed=true end
    if points then
        local old=e.widget.points
        local differs=type(old)~='table' or #old~=#points
        if not differs then
            for i,p in ipairs(points) do
                if math.abs(old[i][1]-p[1])>.1 or math.abs(old[i][2]-p[2])>.1 then differs=true; break end
            end
        end
        if differs then e.widget.points=points; changed=true end
    end
    if changed then ecs.set(id,'UiElement',e) end
end
local function rect(x,y,w,h,scale) return {x=x,y=y,width=w,height=h,scale=scale} end
local function put(name,x,y,w,h,scale,alpha,text,visible)
    local r=rect(x,y,w,h,scale)
    -- scale affects type only, not the serialized rectangle.
    local type_scale=r.scale; r.scale=nil
    local e=assert(ecs.get(ids[name],'UiElement')); local b=base[name]
    if visible==false then
        if e.visible then e.visible=false; ecs.set(ids[name],'UiElement',e) end
        return
    end
    local changed=e.visible~=true; e.visible=true
    for k,v in pairs(r) do if math.abs(e.rect[k]-v)>.05 then e.rect[k]=v; changed=true end end
    for _,field in ipairs({'fill','text','border'}) do
        local value=math.floor(b[field]*alpha+.5)
        if e.style[field][4]~=value then e.style[field][4]=value; changed=true end
    end
    local size=math.max(1,b.font_size*type_scale)
    if math.abs(e.style.font_size-size)>.05 then e.style.font_size=size; changed=true end
    if text and e.widget.text~=text then e.widget.text=text; changed=true end
    if changed then ecs.set(ids[name],'UiElement',e) end
end
local function icon(name,x,y,scale,alpha,vertices,visible)
    local points=ecs.array()
    for i,vertex in ipairs(vertices) do
        local point=ecs.array(); point[1],point[2]=vertex[1]*scale,vertex[2]*scale; points[i]=point
    end
    set_part(name,{x=x,y=y,width=20*scale,height=20*scale},alpha,nil,visible,points)
end
local function slot_style(name,selected)
    local element=ecs.get(ids[name],'UiElement'); local b=base[name]
    local color=selected and {225,193,120} or {245,245,240}
    local changed=false
    for i=1,3 do if element.style.border[i]~=color[i] then element.style.border[i]=color[i]; changed=true end end
    b.border=selected and 225 or 55
    b.fill=selected and 160 or 105
    if changed then ecs.set(ids[name],'UiElement',element) end
end
local function sample_speed(dt)
    local camera=read_camera()
    if not camera or type(camera.position)~='table' then last_position=nil; speed_mps=0; return end
    local p=camera.position
    if not (finite(p[1]) and finite(p[2]) and finite(p[3])) then return end
    if last_position and finite(dt) and dt>0 then
        local dx,dz=p[1]-last_position[1],p[3]-last_position[3]
        local rig=read_rig() or {}
        local units=num(rig.units_per_meter,1,.001,10000)
        local instant=clamp(math.sqrt(dx*dx+dz*dz)/(dt*units),0,100)
        speed_mps=speed_mps+(instant-speed_mps)*(1-math.exp(-math.min(dt,.25)*9))
    end
    last_position={p[1],p[2],p[3]}
end
local function fraction(component)
    if type(component)~='table' or not finite(component.current) then return nil,nil end
    local current=clamp(component.current,0,1000000)
    local maximum=num(component.maximum,100,.01,1000000)
    return math.floor(current+.5),clamp(current/maximum,0,1)
end
-- Magazine positions are authored in source-frame pixels. The tracks are
-- interpolated between measured frames and end where the magazine is hidden.
local function magazine_anchor(clip,index)
    local track=type(annotations.tracks)=='table' and annotations.tracks[clip]
    if type(track)~='table' or #track==0 or not finite(index) or
       index<track[1][1] or index>track[#track][1] then return nil end
    for i=2,#track do
        local a,b=track[i-1],track[i]
        if index<=b[1] then
            local t=clamp((index-a[1])/math.max(1,b[1]-a[1]),0,1)
            return a[2]+(b[2]-a[2])*t,a[3]+(b[3]-a[3])*t
        end
    end
    return track[1][2],track[1][3]
end
local function projected_magazine(viewport,pose)
    if type(pose)~='table' or pose.visible~=true or (finite(pose.opacity) and pose.opacity<=.05) then return nil end
    local x,y=magazine_anchor(pose.clip,pose.index or 0)
    local blend=num(pose.blend,1,0,1)
    if type(pose.previous)=='table' and blend<1 then
        local px,py=magazine_anchor(pose.previous.clip,pose.previous.index or 0)
        if px and x then x,y=px+(x-px)*blend,py+(y-py)*blend
        elseif blend<.5 then x,y=px,py end
    end
    if not x then return nil end
    local dimensions=type(annotations.frame_size)=='table' and annotations.frame_size or {1280,720}
    local frame_width=num(dimensions[1],1280,1,16384)
    local frame_height=num(dimensions[2],720,1,16384)
    local source,aspect=frame_width/frame_height,num(viewport.scene_aspect,16/9,.2,5)
    local u,v=x/frame_width,y/frame_height
    local offset=num(pose.vertical_offset,0,-.25,.25)
    local mx,my=num(pose.motion_x,0,-.2,.2),num(pose.motion_y,0,-.5,.5)
    local angle=num(pose.motion_roll,0,-.2,.2)
    local c,s=math.cos(angle),math.sin(angle)
    local x,y=u-.5,v+offset-1
    local px=c*x-s*y+.5+mx
    local py=s*x+c*y+1+my
    if aspect>source then px=.5+(px-.5)*source/aspect
    else py=1+(py-1)*aspect/source end
    local output_aspect=viewport.width/viewport.height
    local ratio=output_aspect/aspect
    local sx,sy=1,1
    if viewport.scaling=='fill' then
        if ratio<1 then sx=ratio else sy=1/ratio end
    else
        if ratio<1 then sy=1/ratio else sx=ratio end
    end
    local ox,oy=0,0
    if type(viewport.uv_scale)=='table' and type(viewport.uv_offset)=='table' then
        sx=num(viewport.uv_scale[1],sx,.00001,100000)
        sy=num(viewport.uv_scale[2],sy,.00001,100000)
        ox=num(viewport.uv_offset[1],0,-1,1)
        oy=num(viewport.uv_offset[2],0,-1,1)
    end
    local screen_x=((px-.5-ox)/sx+.5)*viewport.width
    local screen_y=((py-.5-oy)/sy+.5)*viewport.height
    if screen_x<0 or screen_x>viewport.width or screen_y<0 or screen_y>viewport.height then return nil end
    return screen_x,screen_y
end
-- Keep screen-edge HUD groups inside the displayed game image, including
-- fit letterboxing, pixel-perfect integer scaling, and centered crops.
local function content_bounds(viewport)
    local w,h=viewport.width,viewport.height
    local aspect=num(viewport.scene_aspect,16/9,.2,5)
    local ratio=(w/h)/aspect
    local sx,sy=1,1
    if viewport.scaling=='fill' then
        if ratio<1 then sx=ratio else sy=1/ratio end
    else
        if ratio<1 then sy=1/ratio else sx=ratio end
    end
    local ox,oy=0,0
    if type(viewport.uv_scale)=='table' and type(viewport.uv_offset)=='table' then
        sx=num(viewport.uv_scale[1],sx,.00001,100000); sy=num(viewport.uv_scale[2],sy,.00001,100000)
        ox=num(viewport.uv_offset[1],0,-1,1); oy=num(viewport.uv_offset[2],0,-1,1)
    end
    local left=clamp((.5+(-.5-ox)/sx)*w,0,w)
    local right=clamp((.5+(.5-ox)/sx)*w,0,w)
    local top=clamp((.5+(-.5-oy)/sy)*h,0,h)
    local bottom=clamp((.5+(.5-oy)/sy)*h,0,h)
    if right<=left or bottom<=top then return 0,0,w,h end
    return left,top,right,bottom
end
-- Only Q inspection asks for a map. Movement and height changes request a
-- fresh slice; the worker keeps the old map usable while a new one is built.
local function request_map(force)
    local cfg=read_hud() or {}
    if cfg.map_enabled==false then map_status='disabled'; return end
    if map_status=='unavailable' then return end
    local camera,rig=read_camera(),read_rig()
    if not camera or type(camera.position)~='table' or not rig then return end
    local p=camera.position
    if not (finite(p[1]) and finite(p[2]) and finite(p[3])) then return end
    local units=num(rig.units_per_meter,1,.5,2.5)
    local body=ecs.get(player,'PhysicsBody') or {}
    local eye=num(rig.eye_height_m,1.7,.7,2.5)*units
    local feet=p[2]-eye
    if finite(body.radius) and finite(body.capsule_half_height) and finite(body.collision_offset_y) then
        feet=p[2]+body.collision_offset_y-body.radius-body.capsule_half_height
    end
    local radius=num(cfg.map_radius_m,12,4,30)*units
    local height=num(cfg.map_height_m,.9144,.1,2)*units
    local thickness=num(cfg.map_thickness_m,.4,.05,.8)*units
    if not force and map_last then
        local dx,dz=p[1]-map_last[1],p[3]-map_last[2]
        if dx*dx+dz*dz < (.8*units)^2 and math.abs(feet-map_last[3])<.3*units and
           radius==map_last[4] and height==map_last[5] and thickness==map_last[6] then return end
    end
    map_last={p[1],p[3],feet,radius,height,thickness}
    if not map_info then map_status='loading' end
    local position=ecs.array(); position[1],position[2],position[3]=p[1],p[2],p[3]
    ecs.emit('hud.map_request',{position=position,feet=feet,units=units,radius=radius,
        height=height,thickness=thickness,size=128})
end
local function refresh(alpha,callout_only)
    current_alpha=alpha
    local viewport=ecs.get_resource('render.viewport') or {}
    local w,h=num(viewport.width,1050,200,10000),num(viewport.height,720,150,10000)
    viewport.width,viewport.height=w,h
    set_ammo_bounds(w,h)
    local scale=clamp(math.min(w/1050,h/720),.62,1.15)
    local left,top,right,bottom=content_bounds(viewport)
    local pose=presentation_seen and presented_pose or ecs.get_resource('hud.weapon_pose')
    local mx,my=projected_magazine(viewport,pose)
    local leader=mx~=nil
    local state=read_weapon() or {}
    local ammo=finite(state.ammo) and math.floor(clamp(state.ammo,0,999)+.5) or nil
    local slot_x=right-158*scale
    local slot_top=bottom-176*scale
    if not callout_only then
    local reserve=finite(state.reserve) and math.floor(clamp(state.reserve,0,9999)+.5) or nil
    local health,hp=fraction(read_health()); local armor,ap=fraction(read_armor())
    local stamina,sp=fraction(ecs.get(player,'Stamina'))
    local controls=read_controls() or {}
    local max_speed=num(controls.move_speed,3,.01,100)*num(controls.sprint_multiplier,1.5,1,10)
    local camera=read_camera() or {}
    local modes={single='Single',burst='Burst',auto='Auto'}
    set_root(true,w,h)
    -- The real voxel slice is north-up. The player marker rotates and moves
    -- against the last completed slice while the worker builds the next one.
    local compass_x,compass_y=left+25*scale,top+22*scale
    local compass_size=132*scale
    local center_x,center_y=compass_x+compass_size*.5,compass_y+compass_size*.5
    local yaw=num(camera.yaw,0,-100000,100000)
    local map_enabled=(read_hud() or {}).map_enabled~=false
    local map_visible=map_enabled and map_status=='ready' and map_info and type(camera.position)=='table'
    local marker_x,marker_y=center_x,center_y
    if map_visible then
        local dx=(camera.position[1]-map_info.center[1])/(2*map_info.radius)*compass_size
        local dy=(camera.position[3]-map_info.center[2])/(2*map_info.radius)*compass_size
        if dx*dx+dy*dy>(compass_size*.43)^2 then map_visible=false
        else marker_x,marker_y=center_x+dx,center_y+dy end
    end
    put('inspect_compass_ring',compass_x,compass_y,compass_size,compass_size,scale,alpha)
    put('inspect_minimap',compass_x,compass_y,compass_size,compass_size,scale,alpha,nil,map_visible)
    local status=map_status=='unavailable' and 'MAP UNAVAILABLE' or map_status=='error' and 'MAP ERROR' or 'MAPPING...'
    put('inspect_map_status',compass_x,center_y+17*scale,compass_size,18*scale,scale,alpha,status,map_enabled and not map_visible)
    local north_x=map_visible and center_x or center_x-math.sin(yaw)*compass_size*.47
    local north_y=map_visible and compass_y+5*scale or center_y-math.cos(yaw)*compass_size*.47
    put('inspect_compass_north',north_x-9*scale,north_y-8*scale,18*scale,16*scale,scale,alpha,'N')
    local angle=map_visible and yaw or 0
    local c,sn=math.cos(angle),math.sin(angle)
    local arrow={}
    for i,p in ipairs({{-6,5},{0,-7},{6,5},{0,2},{-6,5}}) do
        arrow[i]={6+p[1]*c-p[2]*sn,7+p[1]*sn+p[2]*c}
    end
    icon('inspect_compass_arrow',marker_x-6*scale,marker_y-7*scale,scale,alpha,arrow)
    local objective=ecs.get_resource('scene.objective')
    local has_objective=type(objective)=='table' and type(objective.title)=='string' and #objective.title>0
    icon('inspect_objective_icon',compass_x,compass_y+compass_size+17*scale,scale,alpha,
        {{8,0},{16,8},{8,16},{0,8},{8,0}},has_objective)
    put('inspect_objective_title',compass_x+24*scale,compass_y+compass_size+14*scale,230*scale,19*scale,scale,alpha,
        has_objective and objective.title or '',has_objective)
    put('inspect_objective_detail',compass_x+24*scale,compass_y+compass_size+34*scale,230*scale,15*scale,scale,alpha,
        has_objective and type(objective.detail)=='string' and objective.detail or '',has_objective)
    -- Health and armor use live ECS values. Stamina is an optional component;
    -- movement speed is measured from the player's actual horizontal motion.
    local row_count=3+(stamina and 1 or 0)
    local row_x=left+28*scale
    local row_y=bottom-(row_count*30+22)*scale
    local bar_x=row_x+77*scale
    local bar_width=110*scale
    local function bar(track,fill,y,value)
        put(track,bar_x,y+9*scale,bar_width,7*scale,scale,alpha)
        put(fill,bar_x,y+9*scale,math.max(.1,bar_width*(value or 0)),7*scale,scale,alpha,nil,value~=nil and value>0)
    end
    put('inspect_health_icon_h',row_x,row_y+8*scale,17*scale,4*scale,scale,alpha)
    put('inspect_health_icon_v',row_x+6.5*scale,row_y+1.5*scale,4*scale,17*scale,scale,alpha)
    put('inspect_health_value',row_x+25*scale,row_y,42*scale,21*scale,scale,alpha,health and tostring(health) or '--')
    bar('inspect_health_track','inspect_health_fill',row_y,hp)
    row_y=row_y+30*scale
    icon('inspect_armor_icon',row_x,row_y+1*scale,scale,alpha,
        {{8,0},{15,3},{14,11},{8,17},{2,11},{1,3},{8,0}})
    put('inspect_armor_value',row_x+25*scale,row_y,42*scale,21*scale,scale,alpha,armor and tostring(armor) or '--')
    bar('inspect_armor_track','inspect_armor_fill',row_y,ap)
    row_y=row_y+30*scale
    icon('inspect_stamina_icon',row_x,row_y+1*scale,scale,alpha,
        {{10,0},{3,9},{8,9},{5,18},{14,6},{9,6},{10,0}},stamina~=nil)
    put('inspect_stamina_value',row_x+25*scale,row_y,42*scale,21*scale,scale,alpha,stamina and tostring(stamina) or '--',stamina~=nil)
    put('inspect_stamina_track',bar_x,row_y+9*scale,bar_width,7*scale,scale,alpha,nil,stamina~=nil)
    put('inspect_stamina_fill',bar_x,row_y+9*scale,math.max(.1,bar_width*(sp or 0)),7*scale,scale,alpha,nil,sp~=nil and sp>0)
    if stamina then row_y=row_y+30*scale end
    icon('inspect_speed_icon',row_x,row_y+2*scale,scale,alpha,
        {{3,1},{10,1},{10,9},{14,12},{18,13},{18,16},{1,16},{1,13},{3,11},{3,1}})
    put('inspect_speed_value',row_x+20*scale,row_y,47*scale,21*scale,scale,alpha,('%.1f'):format(speed_mps))
    bar('inspect_speed_track','inspect_speed_fill',row_y,clamp(speed_mps/max_speed,0,1))
    -- The current scene has one weapon. Optional Inventory.slots can provide
    -- additional real entries; absent slots remain explicitly empty.
    local inventory=ecs.get(player,'Inventory')
    local slots=type(inventory)=='table' and type(inventory.slots)=='table' and inventory.slots or nil
    local selected=slots and math.floor(num(inventory.active_slot,1,1,4)) or 1
    for i=1,4 do
        local entry=slots and slots[i] or (not slots and i==1 and {name='Rifle',kind='rifle'} or nil)
        local exists=type(entry)=='table' and type(entry.name)=='string' and #entry.name>0
        local name=exists and entry.name or 'Empty'
        if exists and finite(entry.count) then name=name..'  '..tostring(math.floor(math.max(0,entry.count))) end
        local active_slot=i==selected and exists and state.state~='noGun'
        local y=slot_top+(i-1)*35*scale
        local box='inspect_slot'..i..'_box'
        slot_style(box,active_slot)
        put(box,slot_x,y,144*scale,32*scale,scale,alpha)
        put('inspect_slot'..i..'_number',slot_x-20*scale,y+5*scale,18*scale,21*scale,scale,alpha,tostring(i))
        put('inspect_slot'..i..'_name',slot_x+12*scale,y+2*scale,125*scale,15*scale,scale,alpha,name)
    end
    local selected_entry=slots and slots[selected] or (not slots and {kind='rifle'} or nil)
    local kind=type(selected_entry)=='table' and selected_entry.kind or nil
    local firearm=state.state~='noGun' and (kind=='rifle' or kind=='sidearm' or kind=='weapon')
    local selected_y=slot_top+(selected-1)*35*scale
    put('inspect_mode',slot_x+12*scale,selected_y+16*scale,50*scale,13*scale,scale,alpha,modes[state.mode] or '--',firearm)
    put('inspect_reserve',slot_x+62*scale,selected_y+16*scale,78*scale,13*scale,scale,alpha,
        reserve and ('/ %d reserve'):format(reserve) or '/ -- reserve',firearm)
    icon('inspect_weapon_icon',slot_x+106*scale,selected_y+4*scale,scale*.7,alpha,
        {{0,6},{6,6},{7,4},{18,4},{20,2},{25,2},{25,5},{31,5},{31,7},{25,7},{24,10},{19,10},{18,15},{15,15},{16,10},{9,10},{6,13},{4,13},{5,9},{0,9},{0,6}},
        firearm and kind=='rifle')
    -- Former card chrome is kept as scene entities for compatibility, hidden.
    for _,name in ipairs({'inspect_card','inspect_accent','inspect_title','inspect_rule','inspect_health_label','inspect_armor_label'}) do
        put(name,0,0,1,1,scale,alpha,nil,false)
    end
    end -- Full stats and equipment layout.
    local count=ammo and tostring(ammo) or '--'
    if leader then
        -- Match the reference: a number just above a horizontal line, then a
        -- short diagonal to a small dot on the magazine. Flip at screen edges.
        local side=mx<160*scale and 1 or -1
        local end_x=clamp(mx+side*148*scale,10*scale,w-10*scale)
        local bend_x=clamp(mx+side*26*scale,10*scale,w-10*scale)
        local line_y=clamp(my-26*scale,34*scale,h-10*scale)
        local label_x=side<0 and end_x+5*scale or end_x-48*scale
        last_ammo_label_x,last_ammo_label_y=clamp(label_x,8*scale,w-52*scale),line_y-30*scale
        put('inspect_rounds',last_ammo_label_x,last_ammo_label_y,50*scale,28*scale,scale,1,count)
        local points=ecs.array()
        for i,p in ipairs({{mx,my},{bend_x,line_y},{end_x,line_y}}) do
            local q=ecs.array(); q[1],q[2]=p[1],p[2]; points[i]=q
        end
        set_part('inspect_leader',{x=0,y=0,width=w,height=h},1,nil,true,points)
        put('inspect_target_dot',mx-3.5*scale,my-3.5*scale,7*scale,7*scale,scale,1,nil,true)
    else
        -- A firing/ADS clip may hide the magazine. Keep the number near its
        -- last tracked position rather than jumping it to a screen corner.
        -- Without a tracked position, use a stable spot near the weapon.
        if active and not last_ammo_label_x then
            put('inspect_rounds',slot_x+100*scale,slot_top+1*scale,36*scale,20*scale,scale*.7,1,count)
        else
            local x=clamp(last_ammo_label_x or left+(right-left)*.42,left+8*scale,right-52*scale)
            local y=clamp(last_ammo_label_y or bottom-78*scale,top+8*scale,bottom-32*scale)
            put('inspect_rounds',x,y,50*scale,28*scale,scale,1,count)
        end
        set_part('inspect_leader',{x=0,y=0,width=w,height=h},alpha,nil,false)
        put('inspect_target_dot',0,0,7*scale,7*scale,scale,alpha,nil,false)
    end
    put('inspect_target_ring',0,0,1,1,scale,alpha,nil,false)
end
-- A worker supplies the last completed map and its world-space origin.
ecs.on('hud.map_result',function(result)
    if type(result)~='table' then return end
    if result.status=='ready' and type(result.center)=='table' and finite(result.center[1]) and
       finite(result.center[2]) and finite(result.radius) and result.radius>0 then
        map_info={center=result.center,radius=result.radius}; map_status='ready'
    elseif result.status=='unavailable' then
        map_info=nil; map_status='unavailable'
    elseif result.status=='error' then
        map_info=nil; map_status='error'
    else return end
    if active then refresh(current_alpha) end
end)
-- Browser hosts report the frame actually installed by the compositor, which
-- may lag the animation request while a streamed frame is still decoding.
ecs.on('hud.weapon_presentation',function(pose)
    if not ammo_visible or type(pose)~='table' then return end
    presentation_seen,presented_pose=true,pose
    refresh(current_alpha,true)
end)
local function stop()
    if not active then return end
    active=false
    last_position,speed_mps,layout_elapsed=nil,0,0
    set_root(false)
    sync_ammo_visibility()
    engine.events.emit('hud.inspect_ended',{player=player})
end
-- All gameplay combat inputs and reusable weapon events dismiss inspection.
-- A held trigger/aim/reload must be released before Q can open it again.
local function held() for _,down in pairs(combat_held) do if down then return true end end return false end
local function player_event(event)
    return type(event)~='table' or ((event.entity==nil or event.entity==weapon or event.entity==player) and
        (event.player==nil or event.player==player))
end
local function hold_combat(name,pressed)
    combat_held[name]=pressed==true or nil
    if pressed then stop() end
    sync_ammo_visibility()
end
ecs.on('input.reset',function()
    combat_held={}; ammo_remaining=0; stop(); sync_ammo_visibility()
end)
ecs.on('input.mouse_button',function(event)
    if type(event)~='table' or not ({left=true,right=true,middle=true})[event.button] then return end
    if not event.pressed or event.gameplay==true then
        if event.button=='left' and event.pressed==true then show_firing_ammo() end
        hold_combat('mouse_'..event.button,event.pressed==true)
    end
end)
ecs.on('weapon.trigger',function(event)
    if player_event(event) then
        if event and event.pressed==true then show_firing_ammo() end
        hold_combat('trigger',event and event.pressed==true)
    end
end)
ecs.on('weapon.aim',function(event) if player_event(event) then hold_combat('aim',event and event.pressed==true) end end)
for _,channel in ipairs({'weapon.shot','weapon.dry_fire'}) do
    ecs.on(channel,function(event) if player_event(event) then show_firing_ammo(); stop() end end)
end
for _,channel in ipairs({'weapon.reload','weapon.equip','weapon.mode','weapon.optic_set','weapon.reload_started','weapon.hit','weapon.die'}) do
    ecs.on(channel,function(event) if player_event(event) then stop(); sync_ammo_visibility() end end)
end
-- Incoming damage to the player is also an interruption, but damage to
-- another entity must not dismiss the player's inspection.
for _,channel in ipairs({'combat.damage','combat.death'}) do
    ecs.on(channel,function(event) if type(event)=='table' and event.target==player then stop() end end)
end
-- Other Lua gameplay systems can dismiss this HUD without depending on input.
engine.events.on('combat.action',function(event) if player_event(event) then stop() end end)
local combat_keys={KeyR=true,KeyV=true,KeyF=true,KeyG=true,Digit1=true,Digit2=true,Digit3=true,Digit4=true}
ecs.on('input.key',function(event)
    if type(event)~='table' then return end
    if combat_keys[event.code] then
        if event.code=='KeyR' and (not event.pressed or event.gameplay==true) then
            hold_combat('reload_key',event.pressed==true)
        elseif event.pressed and event.gameplay==true then stop() end
        return
    end
    local cfg=read_hud()
    local state=read_weapon() or {}
    local busy=state.state=='reload' or state.state=='reloadADS' or state.state=='repack' or state.state=='dead' or state.state=='death'
    if event.code=='KeyQ' and event.pressed==true and event.gameplay==true and not event.is_repeat and
       not ecs.get_resource('input.binding_capture') and cfg and cfg.enabled~=false and not held() and not busy then
        local was_active=active
        active,age=true,0
        sync_ammo_visibility()
        if not was_active then
            last_position,speed_mps,layout_elapsed=nil,0,0
            engine.events.emit('hud.inspect_started',{player=player})
        end
        map_elapsed=0; request_map(true)
        engine.events.emit('player.inspect',{player=player,weapon=weapon})
        refresh(.01)
    end
end)
-- Timers and visibility are Lua-owned; hidden ammo does no layout work.
ecs.system('hud.inspection',function(dt)
    -- Start the post-fire timeout only once the last trigger is released.
    if not firing_held() then ammo_remaining=math.max(0,ammo_remaining-num(dt,0,0,.25)) end
    sync_ammo_visibility()
    if not active then if ammo_visible then refresh(1,true) end; return end
    local cfg=read_hud()
    if not cfg or cfg.enabled==false then stop(); refresh(1,true); return end
    sample_speed(dt)
    map_elapsed=map_elapsed+num(dt,0,0,.25)
    if map_elapsed>=.25 then map_elapsed=0; request_map(false) end
    local duration=num(cfg.duration_s,2.8,.5,10)
    local fade_in=num(cfg.fade_in_s,.12,.01,1)
    local fade_out=num(cfg.fade_out_s,.35,.01,1)
    age=age+num(dt,0,0,.25)
    if age>=duration then stop(); refresh(1,true); return end
    local t=clamp(math.min(age/fade_in,(duration-age)/fade_out),0,1)
    local alpha=t*t*(3-2*t)
    layout_elapsed=layout_elapsed+num(dt,0,0,.25)
    -- Keep the magazine synced to each presented frame. Update the largely
    -- static status groups at 20 Hz once their fade is complete.
    if t<.9999 or layout_elapsed>=.05 then
        layout_elapsed=0; refresh(alpha)
    else refresh(alpha,true) end
end)
