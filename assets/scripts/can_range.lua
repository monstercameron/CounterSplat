-- Can range: a half wall with a row of cans that are real rigid bodies. Lua places them beside the training range
-- (relative to the player's view, on the probed floor), gives each a hitbox so bullets find them, describes their
-- shapes and materials in the `props.layout` resource, and turns hits into `prop.impulse` events. Rust simulates and
-- draws the props (countersplat_physics::prop_runtime) and writes the cans' positions back to their Transforms.
local ids=assert(ecs.get_resource('scene.entities'))
local player,owner=assert(ids.player),ids.can_range
local config=ecs.get_resource('can_range') or {}
local level=ecs.get_resource('scene.level') or {}
local allowed=type(config.level_ids)~='table' or #config.level_ids==0
if not allowed then
    for _,id in ipairs(config.level_ids) do if id==level.id then allowed=true; break end end
end

local function number(v,fallback,low,high)
    if type(v)~='number' or v~=v or math.abs(v)>10000000 then return fallback end
    return math.max(low,math.min(high,v))
end
local function vec(x,y,z) local a=ecs.array(); a[1],a[2],a[3]=x,y,z; return a end
local function color(v,fallback)
    if type(v)~='table' or #v~=3 then return vec(fallback[1],fallback[2],fallback[3]) end
    return vec(number(v[1],fallback[1],0,8),number(v[2],fallback[2],0,8),number(v[3],fallback[3],0,8))
end
local wall_cfg=type(config.wall)=='table' and config.wall or {}
local can_cfg=type(config.can)=='table' and config.can or {}
local count=math.floor(number(config.can_count,10,1,40))
local palette=type(config.can_colors)=='table' and #config.can_colors>0 and config.can_colors or {{0.75,0.04,0.03}}

local wall,cans,can_by_id={},{},{}
local placed,pending,probe_sequence,version,elapsed=false,nil,0,0,0

local function spawn_entities()
    if wall.id then return end
    wall.id=ecs.spawn(); if owner then ecs.set_parent(wall.id,owner) end
    for i=1,count do
        local id=ecs.spawn(); if owner then ecs.set_parent(id,owner) end
        cans[i]={id=id}; can_by_id[id]=cans[i]
    end
end

-- Builds the layout around the wall's base point (x, floor, z), its long axis (ax, az) pointing across the
-- player's view, and publishes it with a new version so the simulation rebuilds and the cans stand up again.
local function build(x,floor,z,ax,az,units)
    spawn_entities()
    local length=number(wall_cfg.length_m,1.6,.3,10)*units
    local height=number(wall_cfg.height_m,1.0,.2,3)*units
    local thickness=number(wall_cfg.thickness_m,.2,.05,2)*units
    local radius=number(can_cfg.radius_m,.033,.01,.5)*units
    local half_height=number(can_cfg.height_m,.122,.02,1)*units*.5
    -- Rapier turns local +x towards (cos a, -sin a) about +y; hitboxes turn it towards (cos h, sin h).
    local prop_yaw,hit_yaw=math.atan(-az,ax),math.atan(az,ax)
    local bodies=ecs.array()
    bodies[1]={shape='box',half_extents=vec(25*units,.5*units,25*units),position=vec(x,floor-.5*units,z),
        friction=number(config.floor_friction,.6,0,2),restitution=.1,visible=false}
    bodies[2]={entity=wall.id,shape='box',half_extents=vec(length*.5,height*.5,thickness*.5),
        position=vec(x,floor+height*.5,z),yaw=prop_yaw,friction=number(wall_cfg.friction,.7,0,2),restitution=.05,
        color=color(wall_cfg.color,{.38,.36,.33})}
    local spacing=count>1 and (length-6*radius)/(count-1) or 0
    for i,can in ipairs(cans) do
        local along=(i-1-(count-1)*.5)*spacing
        local p=vec(x+ax*along,floor+height+half_height+.002*units,z+az*along)
        can.spawn=p
        bodies[#bodies+1]={entity=can.id,shape='cylinder',radius=radius,half_height=half_height,position=p,
            yaw=(i*1.7)%6.283,dynamic=true,mass=number(can_cfg.mass_kg,.015,.001,10),
            friction=number(can_cfg.friction,.4,0,2),restitution=number(can_cfg.restitution,.25,0,1),
            linear_damping=number(can_cfg.linear_damping,.05,0,10),angular_damping=number(can_cfg.angular_damping,.4,0,10),
            color=color(palette[(i-1)%#palette+1],{.75,.04,.03}),cap_color=color(can_cfg.cap_color,{.75,.75,.72})}
        ecs.set(can.id,'Transform',{position=p})
        -- A sphere hitbox ignores the can's rotation; it just covers the can.
        ecs.set(can.id,'Hitbox',{enabled=true,radius=math.sqrt(radius*radius+half_height*half_height),offset=vec(0,0,0)})
    end
    ecs.set(wall.id,'Transform',{position=vec(x,floor,z)})
    ecs.set(wall.id,'Hitbox',{enabled=true,anchor='feet',units_per_meter=units,yaw=hit_yaw,
        zones={{name='wall',center=vec(0,height*.5/units,0),half_extents=vec(length*.5/units,height*.5/units,thickness*.5/units)}}})
    version=version+1
    ecs.set_resource('props.layout',{version=version,gravity=vec(0,-9.81*units,0),bodies=bodies})
    placed=true
end

-- Places the wall `forward_m` ahead and `lateral_m` to the right of the player's view, facing the player, then
-- builds once the floor under it is known.
local function place()
    local camera=ecs.get(player,'PlayerCamera')
    local rig=ecs.get(player,'PlayerRig')
    local body=ecs.get(player,'PhysicsBody')
    if not camera or type(camera.position)~='table' or not rig then return false end
    local units=number(rig.units_per_meter,1,.01,100)
    local yaw=number(camera.yaw,0,-100000,100000)
    local forward=number(config.forward_m,3.5,1,30)*units
    local side=number(config.lateral_m,3,-20,20)*units
    local feet=camera.position[2]-number(rig.eye_height_m,1.7,.1,10)*units
    if body and type(body.collision_offset_y)=='number' and type(body.radius)=='number' and
       type(body.capsule_half_height)=='number' then
        feet=camera.position[2]+body.collision_offset_y-body.radius-body.capsule_half_height
    end
    local sn,cs=math.sin(yaw),math.cos(yaw)
    local x=camera.position[1]+sn*forward+cs*side
    local z=camera.position[3]-cs*forward+sn*side
    -- Face the player: the wall's long axis is perpendicular to the line from the wall to the eye.
    local dx,dz=camera.position[1]-x,camera.position[3]-z
    local d=math.max(math.sqrt(dx*dx+dz*dz),1e-6)
    local ax,az=-dz/d,dx/d
    if ecs.get_resource('voxel.capture_movement')==true then
        probe_sequence=probe_sequence+1
        pending={token='can-range-ground-'..probe_sequence,x=x,z=z,ax=ax,az=az,units=units,feet=feet}
        spawn_entities()
        engine.physics.probe_ground(wall.id,vec(x,feet+2*units,z),math.min(.1*units,1),math.min(8,4*units),pending.token)
    else
        build(x,feet,z,ax,az,units)
    end
    return true
end

ecs.on('physics.ground_result',function(event)
    if type(event)~='table' or not pending or event.token~=pending.token then return end
    local p=pending; pending=nil
    local floor=type(event.height)=='number' and event.height==event.height and math.abs(event.height)<10000000 and event.height or p.feet
    build(p.x,floor,p.z,p.ax,p.az,p.units)
end)

-- T stands the cans back up where the range is. The spawn placement is chosen for a clear line of fire past the
-- barn's braces, so the wall stays put rather than following the view the way the training targets do.
local function reset()
    if not allowed then return end
    if placed then ecs.emit('prop.reset',{}) elseif not pending then place() end
end
ecs.on('input.key',function(event)
    if type(event)=='table' and event.code=='KeyT' and event.pressed==true and
       event.gameplay==true and not event.is_repeat and not ecs.get_resource('input.binding_capture') then
        reset()
    end
end)
engine.events.on('training.reset',reset)

-- The impact carries the bullet's velocity as it hit. (Reading it from projectile.spawned instead is unreliable: a
-- host may deliver a same-tick spawn and impact in either order.)
ecs.on('projectile.impact',function(event)
    if type(event)~='table' then return end
    local velocity=event.velocity
    local can=can_by_id[event.target]
    if not can or type(velocity)~='table' or type(event.position)~='table' then return end
    local vx,vy,vz=number(velocity[1],0,-1e6,1e6),number(velocity[2],0,-1e6,1e6),number(velocity[3],0,-1e6,1e6)
    local speed=math.sqrt(vx*vx+vy*vy+vz*vz)
    if speed<1e-6 then return end
    local units=number((ecs.get(player,'PlayerRig') or {}).units_per_meter,1,.01,100)
    -- A bullet passing through a thin can leaves a fraction of its momentum behind (kg·m/s), in level units.
    -- Projectiles carry their mass from the weapon profile; `shot_impulse_ns` covers ones that do not.
    local momentum
    if type(event.mass_kg)=='number' and event.mass_kg==event.mass_kg and event.mass_kg>0 then
        momentum=number(event.mass_kg,0,0,1)*speed/units*number(config.momentum_transfer,.03,0,1)
    else
        momentum=number(config.shot_impulse_ns,.12,0,10)
    end
    local impulse=momentum*units/speed
    -- Push at the hit point pulled towards the can's axis, so the hitbox sphere's surface does not exaggerate spin.
    local centre=(ecs.get(event.target,'Transform') or {}).position or event.position
    local point=vec(centre[1]+(event.position[1]-centre[1])*.5,centre[2]+(event.position[2]-centre[2])*.5,
        centre[3]+(event.position[3]-centre[3])*.5)
    ecs.emit('prop.impulse',{entity=event.target,impulse=vec(vx*impulse,vy*impulse,vz*impulse),point=point})
end)

ecs.system('can_range',function(dt)
    if not allowed or placed or pending then return end
    elapsed=elapsed+number(dt,0,0,.25)
    -- Same start as the training targets: after the spawn view settles and collision can probe the floor.
    if elapsed>=.25 and (ecs.get_resource('player.spawn_probe_pending')~=true or elapsed>=2) and
       (ecs.get_resource('voxel.capture_movement')==true or elapsed>=3) then
        place()
    end
end)
