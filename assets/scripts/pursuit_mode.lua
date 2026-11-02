-- Pursuit is ordinary scene-owned gameplay. It uses reusable ECS health,
-- hitbox, projectile and collision APIs; the host only presents snapshots.
local ids=assert(ecs.get_resource('scene.entities'))
local player,weapon,owner=assert(ids.player),assert(ids.first_person),assert(ids.pursuit_mode)
local read_config=engine.component_reader(owner,'PursuitConfig')
local read_level=ids.active_level and engine.component_reader(ids.active_level,'VoxelCollider')
local read_level_state=ids.active_level and engine.component_reader(ids.active_level,'VoxelColliderState')
local defaults={enabled=true,max_targets=4,spawn_interval_s=2,spawn_distance_m=6,speed_mps=1.4,
    target_health=100,zone_health=100,attack_damage=18,attack_interval_s=1.1,attack_range_m=1.1}
local entries={
    {'pursuit_enabled','enabled','Pursuit mode available'},
    {'pursuit_limit','max_targets','Maximum targets',1,4},
    {'pursuit_spawn_rate','spawn_interval_s','Spawn interval (s)',.25,10},
    {'pursuit_spawn_distance','spawn_distance_m','Spawn distance (m)',3,20},
    {'pursuit_speed','speed_mps','Target speed (m/s)',.1,6},
    {'pursuit_health','target_health','Target health',1,1000},
    {'pursuit_zone_health','zone_health','Hitbox health',1,1000},
    {'pursuit_attack_damage','attack_damage','Attack damage',0,100},
    {'pursuit_attack_rate','attack_interval_s','Attack interval (s)',.2,10},
    {'pursuit_attack_range','attack_range_m','Attack range (m)',.3,3},
}
local function number(v,fallback,low,high)
    if type(v)~='number' or v~=v or math.abs(v)>1000000 then return fallback end
    return math.max(low,math.min(high,v))
end
local initial=assert(ecs.get(owner,'PursuitConfig'))
for _,e in ipairs(entries) do
    local field,value=e[2],initial[e[2]]
    if e[4] then value=number(value,defaults[field],e[4],e[5])
    elseif type(value)~='boolean' then value=defaults[field] end
    initial[field]=value
end
ecs.set(owner,'PursuitConfig',initial)
for _,e in ipairs(entries) do
    local spec={component='PursuitConfig',field=e[2],label=e[3]}
    if e[4] then spec.min,spec.max=e[4],e[5] end
    engine.export(owner,e[1],spec)
end
local function cfg(name,low,high)
    local value=read_config()
    return number(value and value[name],defaults[name],low,high)
end
local function vec(x,y,z) local v=ecs.array();v[1],v[2],v[3]=x,y,z;return v end
local function enabled_collision()
    local collider=read_level and read_level()
    local level=read_level_state and read_level_state()
    return collider and collider.enabled and collider.physics and level and level.status=='ready'
end
local function capture() return ecs.get_resource('voxel.capture_movement')==true end
local targets,lookup={},{}
local saved_training=nil
local state={mode='pursuit',status='ready',kills=0,shots=0,hits=0,spawned=0,elapsed=0,alive=0}
local next_spawn,spawn_sequence,token_sequence=0,0,0
local publish_elapsed,visual_elapsed,hud_elapsed,hit_notice=0,0,0,0
local last_state=nil
local function set_ui(id,fields)
    local e=id and ecs.get(id,'UiElement')
    if not e then return end
    local changed=false
    if fields.visible~=nil and e.visible~=fields.visible then e.visible=fields.visible;changed=true end
    if fields.text~=nil and e.widget.text~=fields.text then e.widget.text=fields.text;changed=true end
    if fields.x and math.abs(e.rect.x-fields.x)>.5 then e.rect.x=fields.x;changed=true end
    if fields.y and math.abs(e.rect.y-fields.y)>.5 then e.rect.y=fields.y;changed=true end
    if fields.color then
        for i=1,4 do if e.style.text[i]~=fields.color[i] then e.style.text[i]=fields.color[i];changed=true end end
    end
    if changed then ecs.set(id,'UiElement',e) end
end
local labels={ids.pursuit_target_label_1,ids.pursuit_target_label_2,ids.pursuit_target_label_3,ids.pursuit_target_label_4}
local function alive_count()
    local count=0
    for _,t in ipairs(targets) do if t.phase=='active' and ecs.is_alive(t.id) then count=count+1 end end
    return count
end
local function publish_state(force)
    state.alive=alive_count()
    local signature=table.concat({state.status,state.kills,state.shots,state.hits,state.spawned,state.alive,math.floor(state.elapsed)},':')
    if force or signature~=last_state then
        last_state=signature
        ecs.set_resource('game.mode_state',{mode=state.mode,status=state.status,kills=state.kills,shots=state.shots,
            hits=state.hits,spawned=state.spawned,alive=state.alive,elapsed=state.elapsed})
    end
end
local function set_hud(visible)
    for _,id in ipairs({ids.pursuit_hud_backdrop,ids.pursuit_hud_title,ids.pursuit_hud_stats,ids.pursuit_hud_detail}) do
        set_ui(id,{visible=visible})
    end
    if not visible then
        set_ui(ids.pursuit_hit_notice,{visible=false})
        for _,id in ipairs(labels) do set_ui(id,{visible=false}) end
    end
end
local function hide_practice()
    if saved_training then return end
    saved_training={}
    local training=ecs.get_resource('training_range') or {}
    for _,name in ipairs(type(training.targets)=='table' and training.targets or {}) do
        local id=ids[name]
        local value=id and ecs.get(id,'TrainingTarget')
        if value then
            saved_training[id]={enabled=value.enabled,visible=value.visible}
            value.enabled,value.visible=false,false
            ecs.set(id,'TrainingTarget',value)
        end
    end
end
local function restore_practice()
    if not saved_training then return end
    for id,saved in pairs(saved_training) do
        if ecs.is_alive(id) then
            local value=ecs.get(id,'TrainingTarget')
            if value then value.enabled,value.visible=saved.enabled,saved.visible;ecs.set(id,'TrainingTarget',value) end
        end
    end
    saved_training=nil
end
local function clear_targets()
    for _,t in ipairs(targets) do if ecs.is_alive(t.id) then ecs.despawn(t.id) end end
    targets,lookup={},{}
    for _,id in ipairs(labels) do set_ui(id,{visible=false}) end
    ecs.emit('hitbox.visuals',{layer='pursuit',segments=ecs.array()})
end
local function restore_player()
    for _,component in ipairs({'Health','Armor'}) do
        local value=ecs.get(player,component)
        if value and type(value.maximum)=='number' then value.current=value.maximum;ecs.set(player,component,value) end
    end
    ecs.emit('combat.reset',{owner=player})
    ecs.emit('weapon.reset',{entity=weapon})
end
local function reset_mode(start)
    clear_targets();restore_practice();restore_player()
    state={mode='pursuit',status=start and 'running' or 'ready',kills=0,shots=0,hits=0,spawned=0,elapsed=0,alive=0}
    next_spawn,spawn_sequence,publish_elapsed,visual_elapsed,hud_elapsed,hit_notice=0,0,0,0,0,0
    if start then hide_practice() end
    set_hud(start);publish_state(true)
end
ecs.on('game.mode_request',function(event)
    if type(event)~='table' or (event.mode~=nil and event.mode~='pursuit') then return end
    if event.action=='start' then
        local config=read_config()
        if config and config.enabled~=false then reset_mode(true) end
    elseif event.action=='reset' then reset_mode(false) end
end)
publish_state(true);set_hud(false)

local function discard(t)
    lookup[t.id]=nil
    for i=#targets,1,-1 do if targets[i]==t then table.remove(targets,i);break end end
    if ecs.is_alive(t.id) then ecs.despawn(t.id) end
    if t.slot then set_ui(labels[t.slot],{visible=false}) end
    visual_elapsed=.1
end
local function slot_for_target()
    local used={}
    for _,t in ipairs(targets) do if t.slot then used[t.slot]=true end end
    for i=1,#labels do if not used[i] then return i end end
end
local function activate(t)
    if not ecs.is_alive(t.id) then return end
    local h=ecs.get(t.id,'Hitbox')
    if not h then discard(t);return end
    h.enabled=true;ecs.set(t.id,'Hitbox',h)
    t.phase,t.pending='active',nil
    ecs.set(t.id,'PursuitTarget',{state='chasing',zone_health=t.zone_health,zone_damage={}})
    state.spawned=state.spawned+1
    visual_elapsed=.1;publish_state(true)
end
local function probe_clearance(t)
    local pose=ecs.get(t.id,'Transform')
    if not pose then discard(t);return end
    token_sequence=token_sequence+1
    t.phase,t.wait,t.token='clearance',0,'pursuit-clear-'..token_sequence
    local p=pose.position
    engine.physics.probe_capsule(t.id,vec(p[1],p[2]+t.offset,p[3]),t.radius,t.half,t.token)
end
local function spawn_target()
    local slot=slot_for_target()
    local camera=ecs.get(player,'PlayerCamera')
    local rig=ecs.get(player,'PlayerRig')
    local template=ecs.get(player,'Hitbox')
    if not slot or not camera or not rig or not template or type(template.zones)~='table' then return false end
    local units=number(rig.units_per_meter,1,.05,4)
    local body=ecs.get(player,'PhysicsBody')
    local feet=camera.position[2]-number(rig.eye_height_m,1.7,.1,10)*units
    if body and type(body.collision_offset_y)=='number' and type(body.radius)=='number' and type(body.capsule_half_height)=='number' then
        feet=camera.position[2]+body.collision_offset_y-body.radius-body.capsule_half_height
    end
    spawn_sequence=spawn_sequence+1
    local offsets={0,.55,-.55,1.05,-1.05,1.55,-1.55,2.2,-2.2,3.14}
    local angle=camera.yaw+offsets[(spawn_sequence-1)%#offsets+1]
    local distance=(cfg('spawn_distance_m',3,20)+((spawn_sequence-1)%3)*.6)*units
    local id=ecs.spawn();ecs.set_parent(id,owner)
    local radius=number(.25*units,.25,.05,1)
    local half=number(.6*units,.6,0,4)
    local offset=number(.9*units,.9,.05,8)
    local t={id=id,slot=slot,phase='pending',units=units,radius=radius,half=half,offset=offset,
        zone_health=cfg('zone_health',1,1000),zone_damage={},move_clock=0,attack_wait=0,blocked=0,avoid=0,sign=1,wait=0}
    targets[#targets+1]=t;lookup[id]=t
    ecs.set(id,'Transform',{position=vec(camera.position[1]+math.sin(angle)*distance,feet,
        camera.position[3]-math.cos(angle)*distance)})
    ecs.set(id,'PhysicsBody',{enabled=true,collision_enabled=true,gravity_enabled=false,radius=radius,
        capsule_half_height=half,collision_offset_y=offset,velocity=vec(0,0,0),gravity=18})
    template.enabled=false;template.anchor='feet';template.follow_crouch=false
    template.scale_from_player_rig=false;template.units_per_meter=units;template.yaw=0
    ecs.set(id,'Hitbox',template)
    local health=cfg('target_health',1,1000)
    ecs.set(id,'Health',{current=health,maximum=health})
    ecs.set(id,'PursuitTarget',{state='spawning',zone_health=t.zone_health,zone_damage={}})
    if capture() then
        token_sequence=token_sequence+1
        t.phase,t.token='ground','pursuit-ground-'..token_sequence
        local p=ecs.get(id,'Transform').position
        engine.physics.probe_ground(id,vec(p[1],p[2]+2*units,p[3]),math.min(1,.1*units),math.min(16,4*units),t.token)
    else activate(t) end
    return true
end
ecs.on('physics.ground_result',function(event)
    local t=type(event)=='table' and lookup[event.entity]
    if not t or t.phase~='ground' or event.token~=t.token then return end
    if type(event.height)~='number' or event.height~=event.height or math.abs(event.height)>1000000 then
        discard(t);next_spawn=math.min(next_spawn,.2);return
    end
    local pose=ecs.get(t.id,'Transform')
    if not pose then discard(t);return end
    pose.position[2]=event.height;ecs.set(t.id,'Transform',pose)
    if capture() then probe_clearance(t) else activate(t) end
end)
ecs.on('physics.clearance_result',function(event)
    local t=type(event)=='table' and lookup[event.entity]
    if not t or t.phase~='clearance' or event.token~=t.token then return end
    if event.clear==true then activate(t) else discard(t);next_spawn=math.min(next_spawn,.2) end
end)

local function distance_to_player(t)
    local pose=ecs.get(t.id,'Transform')
    local camera=ecs.get(player,'PlayerCamera')
    if not pose or not camera then return nil end
    local dx,dz=camera.position[1]-pose.position[1],camera.position[3]-pose.position[3]
    return math.sqrt(dx*dx+dz*dz),dx,dz,pose,camera
end
local function attack(t)
    if state.status~='running' or not lookup[t.id] then return end
    local distance,_,_,pose,camera=distance_to_player(t)
    if not distance or distance>cfg('attack_range_m',.3,3)*t.units+.15*t.units or
       math.abs(camera.position[2]-pose.position[2])>2.5*t.units then return end
    local health=ecs.get(player,'Health')
    if not health or type(health.current)~='number' or health.current<=0 then return end
    local damage=cfg('attack_damage',0,100)
    local armor=ecs.get(player,'Armor')
    local absorbed=0
    if armor and type(armor.current)=='number' and armor.current>0 then
        absorbed=math.min(armor.current,damage*number(armor.absorption,.5,0,1))
        armor.current=math.max(0,armor.current-absorbed);ecs.set(player,'Armor',armor)
    end
    local before=health.current
    health.current=math.max(0,before-(damage-absorbed));ecs.set(player,'Health',health)
    ecs.emit('combat.damage',{target=player,source=t.id,owner=t.id,amount=before-health.current+absorbed,
        damage=damage,health_damage=before-health.current,armor_absorbed=absorbed,remaining=health.current})
    if before>0 and health.current<=0 then
        state.status='defeated'
        ecs.emit('combat.death',{target=player,source=t.id,owner=t.id})
        ecs.emit('weapon.die',{})
        publish_state(true)
    end
end
ecs.on('physics.trace_result',function(event)
    local t=type(event)=='table' and lookup[event.entity]
    if not t or not t.attack_token or event.token~=t.attack_token then return end
    t.attack_token=nil
    if event.status=='miss' or event.status=='unavailable' then attack(t) end
end)
ecs.on('physics.motion_resolved',function(event)
    local t=type(event)=='table' and lookup[event.entity]
    if not t or t.phase~='active' or type(event.attempted)~='table' or type(event.moved)~='table' then return end
    local ax,az=number(event.attempted[1],0,-100,100),number(event.attempted[3],0,-100,100)
    local mx,mz=number(event.moved[1],0,-100,100),number(event.moved[3],0,-100,100)
    local attempted=math.sqrt(ax*ax+az*az)
    local moved=math.sqrt(mx*mx+mz*mz)
    if attempted>.005*t.units and moved<attempted*.25 then
        t.blocked=t.blocked+.05
        if t.blocked>=.3 then t.sign=-t.sign;t.avoid=.8;t.blocked=0 end
    else t.blocked=0 end
end)
ecs.on('weapon.shot',function(event)
    if state.status=='running' and type(event)=='table' and event.player==player then state.shots=state.shots+1;publish_state(true) end
end)
ecs.on('combat.damage',function(event)
    local t=type(event)=='table' and lookup[event.target]
    if not t or t.phase~='active' or event.owner~=player then return end
    state.hits=state.hits+1
    local zone=type(event.zone)=='string' and event.zone or 'body'
    local damage=number(event.damage,number(event.health_damage,0,0,1000),0,1000)
    t.zone_damage[zone]=math.min(t.zone_health,(t.zone_damage[zone] or 0)+damage)
    ecs.set(t.id,'PursuitTarget',{state='chasing',zone_health=t.zone_health,zone_damage=t.zone_damage})
    local remaining=number(event.remaining,0,0,100000)
    set_ui(ids.pursuit_hit_notice,{visible=true,text=string.format('−%d  %s  ·  BOX %d/%d  ·  %d HP',math.floor(damage+.5),zone:upper(),math.ceil(t.zone_health-t.zone_damage[zone]),math.ceil(t.zone_health),math.ceil(remaining))})
    hit_notice=1.1;visual_elapsed=.1;publish_state(true)
end)
ecs.on('combat.death',function(event)
    if type(event)=='table' and event.target==player and state.status=='running' then
        state.status='defeated'
        ecs.emit('weapon.die',{})
        publish_state(true)
        return
    end
    local t=type(event)=='table' and lookup[event.target]
    if not t then return end
    if event.owner==player and state.status=='running' then state.kills=state.kills+1 end
    discard(t);next_spawn=math.min(next_spawn,.35);publish_state(true)
end)

local function project(camera,viewport,position)
    if type(viewport)~='table' or type(camera.position)~='table' then return nil end
    local width=number(viewport.width,1050,1,10000)
    local height=number(viewport.height,720,1,10000)
    local aspect=number(viewport.scene_aspect,width/height,.1,10)
    local yaw=number(camera.yaw,0,-10000,10000)
    local pitch=number(camera.pitch,0,-1.55,1.55)+number(camera.view_bob_pitch,0,-1,1)
    local roll=number(camera.roll,0,-10,10)+number(camera.view_bob_roll,0,-1,1)+number(camera.view_turn_roll,0,-1,1)
    local sy,cy,sp,cp,sr,cr=math.sin(yaw),math.cos(yaw),math.sin(pitch),math.cos(pitch),math.sin(roll),math.cos(roll)
    local o=type(camera.view_offset)=='table' and camera.view_offset or {0,0,0}
    local dx=position[1]-camera.position[1]-(o[1] or 0)
    local dy=position[2]-camera.position[2]-(o[2] or 0)-number(camera.view_posture_offset_y,0,-10,10)
    local dz=position[3]-camera.position[3]-(o[3] or 0)
    local forward=dx*sy*cp+dy*sp-dz*cy*cp
    if forward<=.1 then return nil end
    local right=dx*cy+dz*sy
    local up=dx*(-sy*sp)+dy*cp+dz*(cy*sp)
    local r=right*cr-up*sr
    local u=up*cr+right*sr
    local fov=number(camera.view_fov_degrees,number(camera.fov_degrees,71,1,179),1,179)
    local tan=math.tan(math.rad(fov)*.5)
    local px=.5+r/(forward*tan*aspect*2)
    local py=.5-u/(forward*tan*2)
    local scale=type(viewport.uv_scale)=='table' and viewport.uv_scale or {1,1}
    local offset=type(viewport.uv_offset)=='table' and viewport.uv_offset or {0,0}
    local sx,sy2=number(scale[1],1,.01,100),number(scale[2],1,.01,100)
    local x=((px-.5-number(offset[1],0,-10,10))/sx+.5)*width
    local y=((py-.5-number(offset[2],0,-10,10))/sy2+.5)*height
    if x<0 or x>width or y<0 or y>height then return nil end
    return x,y
end
local function update_hud()
    if state.status=='ready' then set_hud(false);return end
    set_hud(true)
    local hp=ecs.get(player,'Health') or {}
    local health=number(hp.current,0,0,100000)
    local gun=ecs.get(weapon,'Ballistics') or {}
    local damage=number(gun.damage,35,0,1000)
    set_ui(ids.pursuit_hud_title,{text=state.status=='defeated' and 'PURSUIT  /  DEFEATED' or 'PURSUIT  /  SURVIVE'})
    set_ui(ids.pursuit_hud_stats,{text=string.format('KILLS %d    ACTIVE %d    HP %d',state.kills,alive_count(),math.ceil(health))})
    set_ui(ids.pursuit_hud_detail,{text=state.status=='defeated' and 'ESC  /  GAME MODE  /  RESTART' or
        string.format('SHOT %d DMG  ·  ESC TO PAUSE',math.floor(damage+.5))})
    local camera=ecs.get(player,'PlayerCamera')
    local viewport=ecs.get_resource('render.viewport') or {width=1050,height=720,scene_aspect=1050/720}
    if hit_notice>0 then set_ui(ids.pursuit_hit_notice,{visible=true,x=number(viewport.width,1050,1,10000)*.5+30,
        y=number(viewport.height,720,1,10000)*.5+48}) end
    for _,t in ipairs(targets) do
        local label=labels[t.slot]
        local pose=t.phase=='active' and ecs.get(t.id,'Transform')
        local hp=t.phase=='active' and ecs.get(t.id,'Health')
        local x,y
        if pose and camera then x,y=project(camera,viewport,{pose.position[1],pose.position[2]+2*t.units,pose.position[3]}) end
        if x and y and hp then
            local ratio=number(hp.current,0,0,100000)/math.max(1,number(hp.maximum,1,1,100000))
            set_ui(label,{visible=true,x=x-50,y=y-22,text=string.format('%d HP',math.ceil(number(hp.current,0,0,100000))),
                color={255,math.floor(80+150*ratio),math.floor(65+110*ratio),255}})
        else set_ui(label,{visible=false}) end
    end
end
local function publish_visuals()
    local boxes=ecs.array()
    for _,t in ipairs(targets) do
        local hitbox=t.phase=='active' and ecs.get(t.id,'Hitbox')
        if hitbox then
            for _,box in ipairs(engine.hitbox.visual_boxes(t.id,hitbox,21)) do
                local damage=number(t.zone_damage[box.zone],0,0,t.zone_health)/t.zone_health
                -- Lower green nonlinearly so a damaged zone reads as red
                -- before the target's final hit removes it from the scene.
                box.color=vec(2.2,.75*(1-damage)*(1-damage)+.02,.18*(1-damage)+.01*damage)
                box.alpha=.9
                boxes[#boxes+1]=box
            end
        end
    end
    ecs.emit('hitbox.visuals',{layer='pursuit',boxes=boxes})
end

ecs.system('game.pursuit',function(dt)
    dt=number(dt,0,0,.25)
    if state.status=='ready' or dt==0 then return end
    if state.status=='running' then state.elapsed=state.elapsed+dt end
    if hit_notice>0 then hit_notice=math.max(0,hit_notice-dt);if hit_notice==0 then set_ui(ids.pursuit_hit_notice,{visible=false}) end end
    local config=read_config()
    if state.status=='running' and config and config.enabled~=false then
        next_spawn=math.max(0,next_spawn-dt)
        local level=read_level_state and read_level_state()
        local waiting=level and level.status=='loading' or (enabled_collision() and not capture())
        if next_spawn<=0 and #targets<math.floor(cfg('max_targets',1,4)) and not waiting then
            if spawn_target() then next_spawn=cfg('spawn_interval_s',.25,10) else next_spawn=.5 end
        end
        for i=#targets,1,-1 do
            local t=targets[i]
            if not ecs.is_alive(t.id) then discard(t)
            elseif t.phase~='active' then
                t.wait=t.wait+dt
                if t.wait>2 then discard(t);next_spawn=math.min(next_spawn,.2) end
            else
                t.move_clock=t.move_clock+dt
                t.attack_wait=math.max(0,t.attack_wait-dt)
                if t.attack_token then
                    t.attack_timeout=(t.attack_timeout or 0)+dt
                    if t.attack_timeout>2 then t.attack_token=nil;t.attack_timeout=0 end
                end
                t.avoid=math.max(0,t.avoid-dt)
                if t.move_clock>=.05 then
                    local step=math.min(t.move_clock,.2);t.move_clock=0
                    local distance,dx,dz,pose,camera=distance_to_player(t)
                    local reach=cfg('attack_range_m',.3,3)*t.units
                    if distance and distance>reach and distance>1e-6 then
                        local nx,nz=dx/distance,dz/distance
                        if t.avoid>0 then nx,nz=nx*.6-nz*.8*t.sign,nz*.6+nx*.8*t.sign end
                        local move=math.min(distance-reach,cfg('speed_mps',.1,6)*t.units*step)
                        engine.physics.move(t.id,vec(nx*move,0,nz*move))
                    elseif distance and distance<=reach and t.attack_wait<=0 and not t.attack_token then
                        t.attack_wait=cfg('attack_interval_s',.2,10)
                        if capture() then
                            token_sequence=token_sequence+1;t.attack_token='pursuit-attack-'..token_sequence;t.attack_timeout=0
                            engine.physics.trace_segment(t.id,vec(pose.position[1],pose.position[2]+1.1*t.units,pose.position[3]),
                                vec(camera.position[1],camera.position[2]-.4*t.units,camera.position[3]),t.attack_token)
                        else attack(t) end
                    end
                end
            end
        end
    end
    visual_elapsed=visual_elapsed+dt;hud_elapsed=hud_elapsed+dt;publish_elapsed=publish_elapsed+dt
    if visual_elapsed>=.05 then visual_elapsed=0;if state.status=='running' then publish_visuals() end end
    if hud_elapsed>=.10 then hud_elapsed=0;update_hud() end
    if publish_elapsed>=.25 then publish_elapsed=0;publish_state(false) end
end)
