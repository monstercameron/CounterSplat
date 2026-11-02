-- Scene-authored shooting range. Lua owns placement, hit feedback, and zone
-- visibility; the host only draws the bounded world-space line snapshot.
local ids=assert(ecs.get_resource('scene.entities'))
local player,notice,notice_bg=assert(ids.player),ids.training_hit_notice,ids.training_hit_backdrop
local config=ecs.get_resource('training_range') or {}
local level=ecs.get_resource('scene.level') or {}
local allowed=type(config.level_ids)~='table' or #config.level_ids==0
if not allowed then
    for _,id in ipairs(config.level_ids) do if id==level.id then allowed=true; break end end
end

local targets,by_id={},{}
for _,name in ipairs(type(config.targets)=='table' and config.targets or {}) do
    local id=ids[name]
    if id and #targets<4 then
        local state={id=id,placed=false,read_settings=engine.component_reader(id,'TrainingTarget'),
            read_hitbox=engine.component_reader(id,'Hitbox'),read_transform=engine.component_reader(id,'Transform')}
        targets[#targets+1]=state
        by_id[id]=state
    end
end
local dirty,elapsed,notice_time,flash_time=true,0,0,0
local flash_target,flash_zone=nil,nil
local probe_sequence,pending_probes=0,{}
local capture_seen=false

local function number(v,fallback,low,high)
    if type(v)~='number' or v~=v or math.abs(v)>10000000 then return fallback end
    return math.max(low,math.min(high,v))
end
local function vec(x,y,z) local a=ecs.array(); a[1],a[2],a[3]=x,y,z; return a end
local function set_notice(text,visible)
    if not notice then return end
    local element=ecs.get(notice,'UiElement')
    if not element then return end
    local viewport=ecs.get_resource('render.viewport') or {}
    local w=number(viewport.width,1050,200,10000)
    local h=number(viewport.height,720,150,10000)
    element.rect.x=w*.5+30
    element.rect.y=h*.5+48
    element.visible=visible
    if text then element.widget.text=text end
    ecs.set(notice,'UiElement',element)
    local background=notice_bg and ecs.get(notice_bg,'UiElement')
    if background then
        background.rect.x,background.rect.y=element.rect.x-8,element.rect.y-2
        background.visible=visible
        ecs.set(notice_bg,'UiElement',background)
    end
end

for _,target in ipairs(targets) do
    for _,entry in ipairs({
        {'target_enabled','enabled','Practice target enabled'},
        {'target_visible','visible','Show hitboxes'},
        {'target_distance','distance_m','Target distance (m)',1.5,30},
        {'target_offset','lateral_m','Target offset (m)',-10,10},
    }) do
        local spec={component='TrainingTarget',field=entry[2],label=entry[3]}
        if entry[4] then spec.min,spec.max=entry[4],entry[5] end
        engine.export(target.id,entry[1],spec)
    end
end

local function probe_floor(target,x,feet,z,units)
    pending_probes[target.id]=nil
    if ecs.get_resource('voxel.capture_movement')~=true then return end
    probe_sequence=probe_sequence+1
    local token='target-ground-'..probe_sequence
    pending_probes[target.id]={token=token,x=x,y=feet,z=z}
    engine.physics.probe_ground(target.id,vec(x,feet+2*units,z),math.min(.1*units,1),math.min(8,4*units),token)
end

local function place(target)
    local camera=ecs.get(player,'PlayerCamera')
    local rig=ecs.get(player,'PlayerRig')
    local body=ecs.get(player,'PhysicsBody')
    local template=ecs.get(player,'Hitbox')
    local cfg=target.read_settings() or {}
    if not camera or type(camera.position)~='table' or not rig or not template then return false end
    local units=number(rig.units_per_meter,1,.01,100)
    local yaw=number(camera.yaw,0,-100000,100000)
    local distance=number(cfg.distance_m,4,1.5,30)*units
    local side=number(cfg.lateral_m,0,-10,10)*units
    local feet=camera.position[2]-number(rig.eye_height_m,1.7,.1,10)*units
    if body and type(body.collision_offset_y)=='number' and type(body.radius)=='number' and
       type(body.capsule_half_height)=='number' then
        feet=camera.position[2]+body.collision_offset_y-body.radius-body.capsule_half_height
    end
    local sn,cs=math.sin(yaw),math.cos(yaw)
    local x=camera.position[1]+sn*distance+cs*side
    local z=camera.position[3]-cs*distance+sn*side
    ecs.set(target.id,'Transform',{position=vec(x,feet,z)})
    template.enabled=cfg.enabled~=false
    template.anchor='feet'
    template.follow_crouch=false
    template.units_per_meter=units
    template.yaw=yaw
    ecs.set(target.id,'Hitbox',template)
    local health=ecs.get(target.id,'Health')
    if health and type(health.maximum)=='number' then
        health.current=health.maximum
        ecs.set(target.id,'Health',health)
    end
    target.placed,dirty=true,true
    -- Refine the player-feet estimate against the level floor when available.
    probe_floor(target,x,feet,z,units)
    return true
end
local function reset_range()
    if not allowed then return end
    for _,target in ipairs(targets) do place(target) end
end

ecs.on('physics.ground_result',function(event)
    if type(event)~='table' then return end
    local pending=pending_probes[event.entity]
    if not pending or event.token~=pending.token then return end
    pending_probes[event.entity]=nil
    if type(event.height)~='number' or event.height~=event.height or math.abs(event.height)>10000000 then return end
    local transform=ecs.get(event.entity,'Transform')
    if not transform or math.abs(transform.position[1]-pending.x)>.001 or
       math.abs(transform.position[2]-pending.y)>.001 or math.abs(transform.position[3]-pending.z)>.001 then return end
    transform.position[2]=event.height
    ecs.set(event.entity,'Transform',transform)
    dirty=true
end)

-- Reposition the range around the player's current facing without touching
-- the level assets or other scenes.
ecs.on('input.key',function(event)
    if type(event)=='table' and event.code=='KeyT' and event.pressed==true and
       event.gameplay==true and not event.is_repeat and not ecs.get_resource('input.binding_capture') then
        reset_range()
    end
end)
engine.events.on('training.reset',reset_range)

ecs.on('projectile.impact',function(event)
    if type(event)~='table' or not by_id[event.target] then return end
    flash_target=event.target
    flash_zone=type(event.zone)=='string' and event.zone or nil
    flash_time,notice_time=.35,1.2
    local label=flash_zone and flash_zone:gsub('_',' '):upper() or 'TARGET'
    local stance=event.stance=='ads' and 'ADS' or event.stance=='hip' and 'HIP' or 'SHOT'
    local distance=number(event.distance_m,0,0,10000)
    local milliseconds=number(event.travel_seconds,0,0,10)*1000
    set_notice(('HIT %s  ·  %s  ·  %.1f m  ·  %.0f ms'):format(label,stance,distance,milliseconds),true)
    dirty=true
end)

local function publish()
    local segments=ecs.array()
    if allowed then
        for _,target in ipairs(targets) do
            local cfg=target.read_settings() or {}
            local hitbox=target.read_hitbox()
            if target.placed and cfg.enabled~=false and cfg.visible~=false and hitbox then
                for _,segment in ipairs(engine.hitbox.visual_segments(target.id,hitbox,21)) do
                    segment.alpha=(flash_time>0 and target.id==flash_target and segment.zone==flash_zone) and 1 or .72
                    segments[#segments+1]=segment
                end
            end
        end
    end
    ecs.emit('hitbox.visuals',{segments=segments})
    dirty=false
end

ecs.system('training.range',function(dt)
    dt=number(dt,0,0,.25)
    elapsed=elapsed+dt
    if notice_time>0 then
        notice_time=math.max(0,notice_time-dt)
        if notice_time==0 then set_notice(nil,false) end
    end
    if flash_time>0 then
        flash_time=math.max(0,flash_time-dt)
        if flash_time==0 then flash_target,flash_zone=nil,nil; dirty=true end
    end
    if allowed then
        local capture=ecs.get_resource('voxel.capture_movement')==true
        if capture and not capture_seen then
            -- Collision may finish loading after the targets first appear.
            for _,target in ipairs(targets) do
                if target.placed then
                    local transform=target.read_transform()
                    local hitbox=target.read_hitbox()
                    if transform and hitbox then
                        local p=transform.position
                        probe_floor(target,p[1],p[2],p[3],number(hitbox.units_per_meter,1,.01,100))
                    end
                end
            end
        end
        capture_seen=capture
        for _,target in ipairs(targets) do
            local cfg=target.read_settings() or {}
            if not target.placed and elapsed>=.25 and
               (ecs.get_resource('player.spawn_probe_pending')~=true or elapsed>=2) then
                if place(target) then
                    target.distance,target.side,target.enabled,target.visible=
                        cfg.distance_m,cfg.lateral_m,cfg.enabled,cfg.visible
                end
            elseif target.placed then
                if cfg.distance_m~=target.distance or cfg.lateral_m~=target.side then
                    target.distance,target.side=cfg.distance_m,cfg.lateral_m
                    place(target)
                end
                if cfg.enabled~=target.enabled or cfg.visible~=target.visible then
                    target.enabled,target.visible=cfg.enabled,cfg.visible
                    local box=ecs.get(target.id,'Hitbox')
                    if box and box.enabled~=(cfg.enabled~=false) then
                        box.enabled=cfg.enabled~=false
                        ecs.set(target.id,'Hitbox',box)
                    end
                    dirty=true
                end
            end
            local transform,hitbox=target.read_transform(),target.read_hitbox()
            -- Transform is an untracked native adapter and may return a fresh
            -- table every tick. Compare its actual position, not table identity.
            local position=transform and transform.position
            local x,y,z=position and position[1],position and position[2],position and position[3]
            if x~=target.last_x or y~=target.last_y or z~=target.last_z or hitbox~=target.last_hitbox then
                target.last_x,target.last_y,target.last_z,target.last_hitbox=x,y,z,hitbox
                dirty=true
            end
        end
    end
    if dirty then publish() end
end)
