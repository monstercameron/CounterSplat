-- Hold-to-crouch behavior for the player ECS entity. Collision math remains a
-- generic engine service; this Lua system owns input, stance and transitions.
local ids = assert(ecs.get_resource('scene.entities'))
local player = assert(ids.player)
if not ecs.get(player, 'PlayerPosture') then return end

for _, item in ipairs({
    {'crouch_enabled','enabled','Crouch enabled'},
    {'crouch_height','crouch_height_ratio','Crouch height ratio',0.4,0.9},
    {'crouch_speed','crouch_speed_multiplier','Crouch speed multiplier',0.1,1},
    {'crouch_transition','transition_seconds','Crouch transition (sec)',0.05,1},
}) do
    local spec={component='PlayerPosture',field=item[2],label=item[3]}
    if item[4] then spec.min,spec.max=item[4],item[5] end
    engine.export(player,item[1],spec)
end
local held = {}
local pending, sequence, retry_after, animation = nil, 0, 0, nil
local last_presentation = nil
local function clamp(n, lo, hi) return math.max(lo, math.min(hi, n)) end
local function number(value, fallback, lo, hi)
    if type(value)~='number' or value~=value then return fallback end
    return clamp(value,lo,hi)
end
local function key_is_down(controls, action)
    return controls and controls.enabled and engine.input.is_down(held,controls.bindings[action])
end
local function wants_crouch(controls, posture)
    return posture.enabled==true and ecs.get_resource('input.binding_capture')~=true and
        key_is_down(controls,'crouch')
end
local function moving(controls)
    if not controls or not controls.enabled then return false end
    local f=(key_is_down(controls,'forward') and 1 or 0)-(key_is_down(controls,'backward') and 1 or 0)
    local r=(key_is_down(controls,'right') and 1 or 0)-(key_is_down(controls,'left') and 1 or 0)
    return f~=0 or r~=0
end
local function set_pending(request)
    pending=request
    ecs.set_resource('player.stand_probe_pending',request~=nil)
end
set_pending(nil)

-- The camera origin is the physical eye. Resizing preserves the feet and moves
-- the eye immediately; a separate render-only offset eases the visible change.
local function resize(posture, crouched, preserve_feet)
    local rig=assert(ecs.get(player,'PlayerRig'))
    local body=assert(ecs.get(player,'PhysicsBody'))
    local camera=assert(ecs.get(player,'PlayerCamera'))
    local old_eye=body.radius+(body.capsule_half_height or 0)-body.collision_offset_y
    local eye_m=rig.eye_height_m
    if crouched then
        local ratio=number(posture.crouch_height_ratio,.6,.4,.9)
        eye_m=math.max(2*rig.radius_m-rig.head_clearance_m,rig.eye_height_m*ratio)
        posture.crouch_eye_height_m=eye_m
    end
    local height=eye_m+rig.head_clearance_m
    local new_eye=eye_m*rig.units_per_meter
    body.radius=rig.radius_m*rig.units_per_meter
    body.capsule_half_height=(height*.5-rig.radius_m)*rig.units_per_meter
    body.collision_offset_y=(height*.5-eye_m)*rig.units_per_meter
    if preserve_feet then
        local delta=new_eye-old_eye
        camera.position[2]=clamp(camera.position[2]+delta,-10000000,10000000)
        camera.view_posture_offset_y=clamp((camera.view_posture_offset_y or 0)-delta,-100,100)
    else
        camera.view_posture_offset_y=0
    end
    ecs.set(player,'PhysicsBody',body)
    ecs.set(player,'PlayerCamera',camera)
    posture.crouched=crouched
    posture.active_units_per_meter=rig.units_per_meter
    posture.blocked=false
    local start=number(posture.amount,0,0,1)
    local target=crouched and 1 or 0
    local duration=number(posture.transition_seconds,.18,.05,1)
    animation={start=start,target=target,elapsed=0,
        duration=math.max(.001,duration*math.abs(target-start)),offset=camera.view_posture_offset_y}
end

local function collision_mode(body)
    if not body or not body.enabled or not body.collision_enabled then return 'free' end
    local level=ids.active_level and ecs.get(ids.active_level,'VoxelCollider')
    local state=ids.active_level and ecs.get(ids.active_level,'VoxelColliderState')
    if not level or not level.enabled or not level.physics then return 'free' end
    if not state or state.status=='loading' then return 'waiting' end
    if state.status=='unavailable' or state.status=='error' then return 'free' end
    if state.status=='ready' and ecs.get_resource('voxel.capture_movement')==true then return 'ready' end
    return 'waiting'
end
local function unchanged(request)
    local camera=ecs.get(player,'PlayerCamera')
    local body=ecs.get(player,'PhysicsBody')
    local rig=ecs.get(player,'PlayerRig')
    if not camera or not body or not rig then return false end
    for i=1,3 do if math.abs(camera.position[i]-request.position[i])>.0001 then return false end end
    return body.radius==request.radius and body.capsule_half_height==request.half_height and
        body.collision_offset_y==request.offset and rig.eye_height_m==request.eye and
        rig.head_clearance_m==request.head and rig.radius_m==request.rig_radius and
        rig.units_per_meter==request.scale
end
local function request_stand(posture)
    local camera=assert(ecs.get(player,'PlayerCamera'))
    local body=assert(ecs.get(player,'PhysicsBody'))
    local rig=assert(ecs.get(player,'PlayerRig'))
    local feet=camera.position[2]+body.collision_offset_y-body.radius-body.capsule_half_height
    local radius=rig.radius_m*rig.units_per_meter
    local height=(rig.eye_height_m+rig.head_clearance_m)*rig.units_per_meter
    local target_half=height*.5-radius
    local current_top=camera.position[2]+body.collision_offset_y+body.capsule_half_height
    local target_top=feet+height-radius
    local center=ecs.array(); center[1]=camera.position[1]; center[3]=camera.position[3]
    local half
    if math.abs(radius-body.radius)<.0001 and target_top>=current_top then
        -- Existing crouched volume is already occupied safely. Check just the
        -- capsule swept by the top sphere to avoid floor-contact false positives.
        center[2]=(current_top+target_top)*.5
        half=(target_top-current_top)*.5
    else
        center[2]=feet+height*.5
        half=target_half
    end
    sequence=sequence+1
    local token='stand-'..sequence
    set_pending({token=token,elapsed=0,position=camera.position,radius=body.radius,
        half_height=body.capsule_half_height,offset=body.collision_offset_y,
        eye=rig.eye_height_m,head=rig.head_clearance_m,rig_radius=rig.radius_m,scale=rig.units_per_meter})
    engine.physics.probe_capsule(player,center,radius,half,token)
end

ecs.on('input.key',function(event)
    if type(event.code)~='string' then return end
    if not event.pressed then held[event.code]=nil
    elseif event.gameplay and not event.is_repeat then held[event.code]=true end
end)
ecs.on('input.reset',function() held={}; set_pending(nil); retry_after=0 end)
ecs.on('physics.clearance_result',function(result)
    if not pending or result.entity~=player or result.token~=pending.token then return end
    local request=pending; set_pending(nil)
    local posture=ecs.get(player,'PlayerPosture')
    local controls=ecs.get(player,'PlayerControls')
    if not posture or not posture.crouched or wants_crouch(controls,posture) then return end
    if result.clear==true and unchanged(request) and collision_mode(ecs.get(player,'PhysicsBody'))=='ready' then
        resize(posture,false,true)
    elseif result.clear~=true and unchanged(request) then
        posture.blocked=true; retry_after=.25
    else
        -- The body or pose changed while the worker was busy: recheck it.
        retry_after=0
    end
    ecs.set(player,'PlayerPosture',posture)
end)
ecs.on('ui.action',function(action)
    if action.id~=ids.reset or action.kind.type~='clicked' then return end
    held={}; set_pending(nil); retry_after=0
    local posture=ecs.get(player,'PlayerPosture')
    if not posture then return end
    -- Reset view teleports to the saved standing spawn; restore the matching body.
    if posture.crouched then resize(posture,false,false) end
    posture.amount=0; posture.moving=false; animation=nil
    ecs.set(player,'PlayerPosture',posture)
end)

ecs.system('player.posture',function(dt)
    local state=ecs.get_many(player, {'PlayerPosture','PlayerControls','PhysicsBody'})
    local posture,controls,body=state.PlayerPosture,state.PlayerControls,state.PhysicsBody
    if not posture or not controls or not body then return end
    local old_amount,old_moving,old_crouched = posture.amount,posture.moving,posture.crouched
    local old_blocked,old_scale,old_eye = posture.blocked,posture.active_units_per_meter,posture.crouch_eye_height_m
    dt=clamp(dt,0,.1)
    retry_after=math.max(0,retry_after-dt)
    local want=wants_crouch(controls,posture)
    if pending then
        pending.elapsed=pending.elapsed+dt
        if want or pending.elapsed>.75 then set_pending(nil); retry_after=.1 end
    end
    local mode=collision_mode(body)
    local spawn_pending=ecs.get_resource('player.spawn_probe_pending')==true
    if want and not posture.crouched and not spawn_pending and mode~='waiting' then
        set_pending(nil); resize(posture,true,true)
    elseif not want and posture.crouched and not pending and not spawn_pending then
        if mode=='free' then resize(posture,false,true)
        elseif mode=='ready' and retry_after<=0 then request_stand(posture)
        else posture.blocked=true end
    end
    if animation then
        animation.elapsed=math.min(animation.duration,animation.elapsed+dt)
        local t=animation.elapsed/animation.duration
        local smooth=t*t*(3-2*t)
        posture.amount=animation.start+(animation.target-animation.start)*smooth
        local camera=ecs.get(player,'PlayerCamera')
        camera.view_posture_offset_y=animation.offset*(1-smooth)
        ecs.set_field(player,'PlayerCamera','view_posture_offset_y',camera.view_posture_offset_y)
        if t>=1 then posture.amount=animation.target; animation=nil end
    end
    posture.moving=moving(controls)
    if not posture.crouched then
        local rig=ecs.get(player,'PlayerRig')
        if rig then posture.active_units_per_meter=rig.units_per_meter end
    end
    if old_amount~=posture.amount or old_moving~=posture.moving or old_crouched~=posture.crouched or
        old_blocked~=posture.blocked or old_scale~=posture.active_units_per_meter or old_eye~=posture.crouch_eye_height_m then
        ecs.set(player,'PlayerPosture',posture)
    end
    local signature=('%0.5f:%s'):format(posture.amount,tostring(posture.moving))
    if signature~=last_presentation then
        last_presentation=signature
        ecs.emit('weapon.posture',{crouch=posture.amount,moving=posture.moving})
    end
end)
