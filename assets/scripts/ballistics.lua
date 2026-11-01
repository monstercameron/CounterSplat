-- Projectile rules are game behavior: this script owns travel, hit selection,
-- damage, and lifetime. The physics worker only answers finite segment queries.
local ids = assert(ecs.get_resource('scene.entities'))
local weapon = assert(ids.first_person)
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
    ui.widget.text=message; ui.visible=true; ecs.set(id,'UiElement',ui)
    notice_time=1.1
end

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

-- Sphere hitboxes are optional ECS data, independent of rendering. Pick the
-- earliest entity hit; the returned voxel result can still be closer.
local function entity_hit(p, a, b)
    local best=nil
    local d=difference(b,a)
    local aa=d[1]*d[1]+d[2]*d[2]+d[3]*d[3]
    if aa<1e-20 then return nil end
    for _,id in ipairs(ecs.query({'Hitbox'})) do
        if id~=p.owner and id~=p.source and id~=p.entity and ecs.is_alive(id) then
            local hitbox=ecs.get(id,'Hitbox')
            local pose=ecs.get(id,'Transform') or ecs.get(id,'PlayerCamera')
            if hitbox and hitbox.enabled~=false and pose and vector(pose.position) and
                type(hitbox.radius)=='number' and hitbox.radius==hitbox.radius and hitbox.radius>0 and hitbox.radius<=1000 then
                local offset=vector(hitbox.offset) and hitbox.offset or {0,0,0}
                local center=vec(pose.position[1]+offset[1],pose.position[2]+offset[2],pose.position[3]+offset[3])
                local m=difference(a,center)
                local mb=m[1]*d[1]+m[2]*d[2]+m[3]*d[3]
                local c=m[1]*m[1]+m[2]*m[2]+m[3]*m[3]-hitbox.radius*hitbox.radius
                local disc=mb*mb-aa*c
                local t=nil
                if c<=0 then t=0 elseif disc>=0 then t=(-mb-math.sqrt(disc))/aa end
                if t and t>=0 and t<=1 and (not best or t<best.fraction) then
                    local point=interpolate(a,b,t)
                    local n=difference(point,center); local size=length(n)
                    if size>1e-9 then n=vec(n[1]/size,n[2]/size,n[3]/size)
                    else local l=math.sqrt(aa); n=vec(-d[1]/l,-d[2]/l,-d[3]/l) end
                    best={fraction=t,position=point,normal=n,target=id}
                end
            end
        end
    end
    return best
end

local function apply_damage(p, target, hit)
    if not target or not ecs.is_alive(target) then return end
    local health=ecs.get(target,'Health')
    if not health or type(health.current)~='number' or health.current~=health.current or health.current<=0 then return end
    local before=health.current
    health.current=math.max(0,before-p.damage)
    ecs.set(target,'Health',health)
    ecs.emit('combat.damage',{target=target,source=p.source,owner=p.owner,amount=before-health.current,
        remaining=health.current,position=hit.position,shot=p.shot})
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
        local event={entity=p.entity,owner=p.owner,source=p.source,shot=p.shot,
            position=copy(position),normal=copy(hit.normal),travel_seconds=p.age,
            distance_m=p.distance/p.units,damage=p.damage,kind=target and 'entity' or 'level'}
        if target then event.target=target end
        ecs.set_resource('combat.last_impact',event)
        ecs.emit('projectile.impact',event)
        notice(('%s  %.1f m  %.0f ms'):format(target and 'TARGET HIT' or 'LEVEL IMPACT',event.distance_m,event.travel_seconds*1000))
        apply_damage(p,target,event)
        finish(p)
    elseif p.age+1e-7>=p.lifetime or p.distance+1e-5>=p.range then
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
    local level=ids.active_level and ecs.get(ids.active_level,'VoxelCollider')
    local state=ids.active_level and ecs.get(ids.active_level,'VoxelColliderState')
    local waiting=level and level.enabled and level.physics and state and state.status=='loading'
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
                        target=entity_hit(p,p.position,endpoint)}
                    if ecs.get_resource('voxel.capture_movement')==true then
                        engine.physics.trace_segment(id,p.position,endpoint,token)
                    else resolve(p,'unavailable') end
                end
            end
        end
    end
    publish_visuals()
end)
