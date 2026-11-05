-- Projectile rules are game behavior: this script owns travel, hit selection,
-- damage, and lifetime. The physics worker only answers finite segment queries.
local ids = assert(ecs.get_resource('scene.entities'))
local weapon = assert(ids.first_person)
local read_level = ids.active_level and engine.component_reader(ids.active_level, 'VoxelCollider')
local read_level_state = ids.active_level and engine.component_reader(ids.active_level, 'VoxelColliderState')
local active, trails = {}, {}
local sequence, had_visuals, notice_time = 0, false, 0
local MAX_PROJECTILES, MAX_TRAILS = 64, 256

local function clamp(n, low, high) return math.max(low, math.min(high, n)) end
local function number(value, fallback, low, high)
    if type(value) ~= 'number' or value ~= value or math.abs(value) > 10000000 then return fallback end
    return clamp(value, low, high)
end
local function vec(x, y, z)
    local a=ecs.array(); a[1],a[2],a[3]=x,y,z; return a
end
local function copy(v) return vec(v[1],v[2],v[3]) end
local function vector(v)
    return type(v)=='table' and #v==3 and type(v[1])=='number' and type(v[2])=='number' and
        type(v[3])=='number' and v[1]==v[1] and v[2]==v[2] and v[3]==v[3] and
        math.abs(v[1])<=10000000 and math.abs(v[2])<=10000000 and math.abs(v[3])<=10000000
end
local function length(v) return math.sqrt(v[1]*v[1]+v[2]*v[2]+v[3]*v[3]) end
local function difference(a,b) return vec(a[1]-b[1],a[2]-b[2],a[3]-b[3]) end
local function interpolate(a,b,t)
    return vec(a[1]+(b[1]-a[1])*t,a[2]+(b[2]-a[2])*t,a[3]+(b[3]-a[3])*t)
end

local cfg=ecs.get(weapon,'Ballistics')
if cfg then
    for _, e in ipairs({
        {'bullet_speed','speed_mps','Bullet speed (m/s)',1,3000,715},
        {'bullet_gravity','gravity_mps2','Bullet gravity (m/s²)',0,100,9.81},
        {'bullet_range','max_range_m','Bullet range (m)',1,5000,1200},
        {'bullet_lifetime','max_lifetime_s','Bullet lifetime (s)',0.05,10,3},
        {'bullet_damage','damage','Bullet damage',0,1000,35},
        {'bullet_tracers','show_tracers','Show shot traces'},
    }) do
        local spec={component='Ballistics',field=e[2],label=e[3]}
        if e[4] then
            spec.min,spec.max=e[4],e[5]
            cfg[e[2]]=number(cfg[e[2]],e[6],e[4],e[5])
        elseif type(cfg[e[2]])~='boolean' then cfg[e[2]]=true end
        -- The inspector exports the actual component field, including defaults.
        ecs.set(weapon,'Ballistics',cfg)
        engine.export(weapon,e[1],spec)
    end
end

local function notice(message)
    local id=ids.weapon_impact_notice
    local ui=id and ecs.get(id,'UiElement')
    if not ui then return end
    notice_time=1.1
    ui.widget.text=message
    ui.visible=ecs.get_resource('hud.editor_visible')==true
    ecs.set(id,'UiElement',ui)
end
-- Debug notices follow the ECS tools, even if toggled while a notice is live.
engine.events.on('hud.editor_visibility_changed',function(event)
    local id=ids.weapon_impact_notice
    local ui=id and ecs.get(id,'UiElement')
    if not ui then return end
    local visible=type(event)=='table' and event.visible==true and notice_time>0
    if ui.visible~=visible then ui.visible=visible; ecs.set(id,'UiElement',ui) end
end)

local function add_trail(a,b,ttl)
    if #trails>=MAX_TRAILS then table.remove(trails,1) end
    trails[#trails+1]={start=copy(a),finish=copy(b),life=ttl,total=ttl}
end
local function publish_visuals()
    if #trails==0 and not had_visuals then return end
    local lines=ecs.array()
    for _,t in ipairs(trails) do
        lines[#lines+1]={start=t.start,finish=t.finish,alpha=clamp(t.life/t.total,0,1)}
    end
    ecs.emit('projectile.visuals',{segments=lines})
    had_visuals=#trails>0
end
local function finish(p, reason)
    active[p.entity]=nil
    if ecs.is_alive(p.entity) then ecs.despawn(p.entity) end
    if reason then
        local result={entity=p.entity,owner=p.owner,shot=p.shot,reason=reason}
        ecs.set_resource('combat.last_expiration',result)
        ecs.emit('projectile.expired',result)
    end
end

-- Cancel in-flight shots on a gameplay reset; late worker replies become harmless.
ecs.on('combat.reset',function(event)
    local owner=type(event)=='table' and event.owner or nil
    for id,projectile in pairs(active) do
        if owner==nil or projectile.owner==owner then finish(projectile) end
    end
    trails={}; publish_visuals()
end)

-- The shared Lua hitbox API returns the closest zone for each target.
local function entity_hit(p, a, b)
    local best=nil
    for _,id in ipairs(ecs.query({'Hitbox'})) do
        if id~=p.owner and id~=p.source and id~=p.entity and ecs.is_alive(id) then
            local hit=engine.hitbox.trace_segment(id,ecs.get(id,'Hitbox'),a,b)
            if hit and (not best or hit.fraction<best.fraction) then best=hit end
        end
    end
    return best
end

-- Closest distance between a projectile segment and the player's upright
-- capsule centerline. Only the already-confirmed part of the path is checked,
-- so a wall hit cannot suppress someone on the other side of the wall.
local function player_capsule()
    local camera=ecs.get(ids.player,'PlayerCamera')
    local rig=ecs.get(ids.player,'PlayerRig')
    local body=ecs.get(ids.player,'PhysicsBody')
    if not camera or not vector(camera.position) then return nil end
    local units=number(rig and rig.units_per_meter,1,.01,100)
    local radius=number(body and body.radius,.25*units,.01,1000)
    local center=camera.position[2]+number(body and body.collision_offset_y,-.85*units,-1000,1000)
    local half=number(body and body.capsule_half_height,.55*units,0,1000)
    return {x=camera.position[1],z=camera.position[3],low=center-half,high=center+half,
        radius=radius,units=units}
end

local function player_proximity(a,b,capsule)
    if not capsule then return nil end
    local x,z,low,high=capsule.x,capsule.z,capsule.low,capsule.high
    local dx,dy,dz=b[1]-a[1],b[2]-a[2],b[3]-a[3]
    local length2=dx*dx+dy*dy+dz*dz
    if length2<1e-20 then return nil end
    local best,best_t=math.huge,0
    local function sample(t)
        t=clamp(t,0,1)
        local px,py,pz=a[1]+dx*t,a[2]+dy*t,a[3]+dz*t
        local vertical=py<low and py-low or py>high and py-high or 0
        local distance=(px-x)^2+vertical*vertical+(pz-z)^2
        if distance<best then best,best_t=distance,t end
    end
    sample(0); sample(1)
    local horizontal=dx*dx+dz*dz
    if horizontal>1e-20 then sample(-((a[1]-x)*dx+(a[3]-z)*dz)/horizontal) end
    -- The nearest point can also lie above/below either end cap.
    for _,y in ipairs({low,high}) do
        sample(-((a[1]-x)*dx+(a[2]-y)*dy+(a[3]-z)*dz)/length2)
    end
    return math.max(0,math.sqrt(best)-capsule.radius)/capsule.units,best_t
end

local function emit_near_miss(p)
    local candidate=p.near_player
    if not candidate or p.suppressed_player then return end
    p.suppressed_player=true
    p.near_player=nil
    ecs.emit('combat.near_miss',{target=ids.player,owner=p.owner,source=p.source,
        projectile=p.entity,shot=p.shot,distance_m=candidate.distance,
        intensity=clamp(1-candidate.distance/candidate.range,.05,1)})
end

local function report_near_miss(p,a,b,hit,capsule)
    if p.suppressed_player or p.owner==ids.player or not ecs.is_alive(ids.player) then return end
    if hit and hit.target==ids.player then
        -- A projectile that was approaching the player is a hit, not a miss.
        p.near_player=nil
        return
    end
    local receiver=ecs.get(ids.player,'SuppressionReceiver')
    if not receiver or receiver.enabled==false then p.near_player=nil; return end
    local range=number(receiver.near_miss_radius_m,1.25,0,5)
    if range<=0 then p.near_player=nil; return end
    local health=ecs.get(ids.player,'Health')
    if health and type(health.current)=='number' and health.current<=0 then p.near_player=nil; return end
    local distance,t=player_proximity(a,b,capsule)
    if not distance then return end
    if distance<=range and (not p.near_player or distance<p.near_player.distance) then
        p.near_player={distance=distance,range=range}
    end
    -- Wait until the projectile passes its closest point, or strikes something
    -- else, so a later direct hit does not also produce a near-miss event.
    if p.near_player and (t<1-1e-7 or hit) then emit_near_miss(p) end
end

local function apply_damage(p, target, hit)
    if not target or not ecs.is_alive(target) then return end
    local health=ecs.get(target,'Health')
    if not health or type(health.current)~='number' or health.current~=health.current or health.current<=0 then return end
    local before=health.current
    local absorbed=0
    local armor=ecs.get(target,'Armor')
    if type(armor)=='table' and type(armor.current)=='number' and armor.current==armor.current and armor.current>0 then
        local rate=type(armor.absorption)=='number' and armor.absorption==armor.absorption and armor.absorption or 0.5
        absorbed=math.min(armor.current,p.damage*math.max(0,math.min(1,rate)))
        armor.current=math.max(0,armor.current-absorbed)
        ecs.set(target,'Armor',armor)
    end
    health.current=math.max(0,before-(p.damage-absorbed))
    ecs.set(target,'Health',health)
    ecs.emit('combat.damage',{target=target,source=p.source,owner=p.owner,amount=before-health.current+absorbed,
        health_damage=before-health.current,damage=p.damage,armor_absorbed=absorbed,armor_remaining=type(armor)=='table' and armor.current or nil,
        remaining=health.current,position=hit.position,shot=p.shot,zone=hit.zone})
    if before>0 and health.current<=0 then
        ecs.emit('combat.death',{target=target,source=p.source,owner=p.owner,shot=p.shot})
    end
end

local function resolve(p, status, voxel)
    local pending=p.pending
    if not pending then return end
    p.pending=nil
    if status=='error' then finish(p,'collision_error'); return end
    local hit=pending.target
    if hit and hit.target and not ecs.is_alive(hit.target) then hit=nil end
    if status=='hit' and voxel and (not hit or voxel.fraction<=hit.fraction) then hit=voxel end
    local fraction=hit and hit.fraction or 1
    local position=hit and hit.position or pending.finish
    report_near_miss(p,pending.start,position,hit,pending.player_capsule)
    if p.tracers then
        add_trail(pending.start,position,0.10)
        if hit then
            local radius=0.055*p.units
            add_trail(vec(position[1]-radius,position[2],position[3]),vec(position[1]+radius,position[2],position[3]),0.20)
            add_trail(vec(position[1],position[2]-radius,position[3]),vec(position[1],position[2]+radius,position[3]),0.20)
            add_trail(vec(position[1],position[2],position[3]-radius),vec(position[1],position[2],position[3]+radius),0.20)
        end
    end
    p.age=p.age+pending.dt*fraction
    p.distance=p.distance+length(difference(position,pending.start))
    p.position=copy(position)
    p.velocity=vec(p.velocity[1],p.velocity[2]-p.gravity*pending.dt*fraction,p.velocity[3])
    p.backlog=math.max(0,p.backlog-pending.dt)
    if hit then
        local target=hit.target
        local event={entity=p.entity,owner=p.owner,source=p.source,shot=p.shot,stance=p.stance,
            position=copy(position),normal=copy(hit.normal),velocity=copy(p.velocity),travel_seconds=p.age,
            distance_m=p.distance/p.units,damage=p.damage,kind=target and 'entity' or 'level'}
        if target then event.target=target; event.zone=hit.zone end
        ecs.set_resource('combat.last_impact',event)
        ecs.emit('projectile.impact',event)
        notice(('%s  %.1f m  %.0f ms'):format(target and 'TARGET HIT' or 'LEVEL IMPACT',event.distance_m,event.travel_seconds*1000))
        apply_damage(p,target,event)
        finish(p)
    elseif p.age+1e-7>=p.lifetime or p.distance+1e-5>=p.range then
        emit_near_miss(p)
        finish(p,p.age+1e-7>=p.lifetime and 'lifetime' or 'range')
    else
        if status=='unavailable' and not p.warned_unavailable then
            p.warned_unavailable=true; notice('Level collision unavailable')
        end
        ecs.set(p.entity,'Transform',{position=copy(p.position)})
        ecs.set(p.entity,'Projectile',{owner=p.owner,source=p.source,shot=p.shot,
            velocity=copy(p.velocity),travel_seconds=p.age,distance_m=p.distance/p.units,damage=p.damage})
    end
end

ecs.on('weapon.shot',function(event)
    if not vector(event.origin) or not vector(event.direction) or type(event.entity)~='string' or
        type(event.player)~='string' or not ecs.is_alive(event.entity) or not ecs.is_alive(event.player) then return end
    local count=0; for _ in pairs(active) do count=count+1 end
    if count>=MAX_PROJECTILES then
        ecs.emit('projectile.expired',{owner=event.player,shot=event.shot,reason='capacity'}); return
    end
    local magnitude=length(event.direction)
    if magnitude<1e-6 then return end
    local settings=ecs.get(event.entity,'Ballistics') or {}
    local rig=ecs.get(event.player,'PlayerRig') or {}
    local units=number(rig.units_per_meter,1,0.01,100)
    local speed=number(settings.speed_mps,715,1,3000)*units
    local id=ecs.spawn()
    local p={entity=id,source=event.entity,owner=event.player,shot=event.shot or 0,units=units,
        stance=(event.stance=='ads' or event.stance=='hip') and event.stance or nil,
        position=copy(event.origin),velocity=vec(event.direction[1]/magnitude*speed,event.direction[2]/magnitude*speed,event.direction[3]/magnitude*speed),
        gravity=number(settings.gravity_mps2,9.81,0,100)*units,
        lifetime=number(settings.max_lifetime_s,3,0.05,10),range=number(settings.max_range_m,1200,1,5000)*units,
        damage=number(settings.damage,35,0,1000),tracers=settings.show_tracers~=false,
        age=0,distance=0,backlog=0,loading_wait=0,pending=nil}
    active[id]=p
    ecs.set_resource('combat.last_shot',{entity=id,owner=p.owner,shot=p.shot,origin=copy(p.position),velocity=copy(p.velocity)})
    ecs.set(id,'Transform',{position=copy(p.position)})
    ecs.set(id,'Projectile',{owner=p.owner,source=p.source,shot=p.shot,velocity=copy(p.velocity),travel_seconds=0,distance_m=0,damage=p.damage})
    ecs.emit('projectile.spawned',{entity=id,owner=p.owner,source=p.source,shot=p.shot,position=copy(p.position),velocity=copy(p.velocity)})
end)

ecs.on('physics.trace_result',function(result)
    local p=active[result.entity]
    if not p or not p.pending or p.pending.token~=result.token then return end
    if result.status=='hit' then
        if type(result.fraction)~='number' or result.fraction<0 or result.fraction>1 or
            not vector(result.position) or not vector(result.normal) then resolve(p,'error'); return end
        resolve(p,'hit',{fraction=result.fraction,position=result.position,normal=result.normal})
    elseif result.status=='miss' or result.status=='unavailable' then resolve(p,result.status)
    else resolve(p,'error') end
end)

ecs.system('combat.projectiles',function(dt)
    dt=number(dt,0,0,0.25)
    if notice_time>0 then
        notice_time=math.max(0,notice_time-dt)
        if notice_time==0 and ids.weapon_impact_notice then
            local ui=ecs.get(ids.weapon_impact_notice,'UiElement')
            if ui then ui.visible=false; ecs.set(ids.weapon_impact_notice,'UiElement',ui) end
        end
    end
    for i=#trails,1,-1 do
        trails[i].life=trails[i].life-dt
        if trails[i].life<=0 then table.remove(trails,i) end
    end
    -- Collision settings matter only while a projectile is in flight.
    local waiting
    if next(active) then
        local level=read_level and read_level()
        local state=read_level_state and read_level_state()
        waiting=level and level.enabled and level.physics and state and state.status=='loading'
    end
    for id,p in pairs(active) do
        if not ecs.is_alive(id) then active[id]=nil
        else
            p.backlog=math.min(p.lifetime-p.age,p.backlog+dt)
            if waiting then
                p.loading_wait=p.loading_wait+dt
                if p.loading_wait>=10 then notice('Level collision timed out'); finish(p,'collision_unavailable') end
            else p.loading_wait=0 end
            if active[id] and p.pending then
                p.pending.wait=p.pending.wait+dt
                if p.pending.wait>=2 then finish(p,'collision_timeout') end
            end
            if active[id] and not p.pending and not waiting and p.backlog>0 then
                local duration=p.backlog
                local function end_at(t)
                    return vec(p.position[1]+p.velocity[1]*t,
                        p.position[2]+p.velocity[2]*t-0.5*p.gravity*t*t,
                        p.position[3]+p.velocity[3]*t)
                end
                local endpoint=end_at(duration)
                local distance=length(difference(endpoint,p.position))
                local remaining=math.max(0,p.range-p.distance)
                if distance>remaining and distance>0 then duration=duration*remaining/distance; endpoint=end_at(duration) end
                if duration<=1e-9 or not vector(endpoint) then finish(p,'range')
                else
                    sequence=sequence+1
                    local token='projectile-'..sequence
                    p.pending={token=token,start=copy(p.position),finish=endpoint,dt=duration,wait=0,
                        target=entity_hit(p,p.position,endpoint),
                        player_capsule=p.owner~=ids.player and not p.suppressed_player and player_capsule() or nil}
                    if ecs.get_resource('voxel.capture_movement')==true then
                        engine.physics.trace_segment(id,p.position,endpoint,token)
                    else resolve(p,'unavailable') end
                end
            end
        end
    end
    publish_visuals()
end)
