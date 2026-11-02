-- World-space impact effects are gameplay data. The renderer receives only a
-- bounded visual snapshot and never decides where an impact occurred.
local ids = assert(ecs.get_resource('scene.entities'))
local entity = assert(ids.impact_effects)
local read_settings = engine.component_reader(entity, 'ImpactEffects')
local marks, smoke = {}, {}
local changed, had_visuals = false, false
local MAX_MARKS, MAX_SMOKE = 96, 160

local function finite(n) return type(n)=='number' and n==n and math.abs(n)<=10000000 end
local function number(n, fallback, lo, hi)
    if not finite(n) then return fallback end
    return math.max(lo, math.min(hi, n))
end
local function vector(v)
    return type(v)=='table' and #v==3 and finite(v[1]) and finite(v[2]) and finite(v[3])
end
local function vec(x,y,z) local a=ecs.array(); a[1],a[2],a[3]=x,y,z; return a end
local function unit(v)
    if not vector(v) then return nil end
    local length=math.sqrt(v[1]*v[1]+v[2]*v[2]+v[3]*v[3])
    if length<0.00001 then return nil end
    return vec(v[1]/length,v[2]/length,v[3]/length)
end
local function fraction(n) return n-math.floor(n) end
local function random(seed) return fraction(math.sin(seed*12.9898+78.233)*43758.5453) end
local function settings() return read_settings() or {} end
local function publish()
    if not changed and #smoke==0 then return end
    if #marks==0 and #smoke==0 and not had_visuals then changed=false; return end
    local decals, particles=ecs.array(),ecs.array()
    for _,mark in ipairs(marks) do
        local fade=math.min(1,math.max(0,(mark.life-mark.age)/math.min(mark.life,3)))
        decals[#decals+1]={position=mark.position,normal=mark.normal,radius=mark.radius,
            opacity=fade*mark.opacity,variation=mark.variation}
    end
    for _,p in ipairs(smoke) do
        local t=math.min(1,p.age/p.life)
        local rise=p.age
        local wobble=math.sin(p.age*4+p.variation)*p.age*.04*p.units
        local opacity=p.opacity*math.min(1,p.age/.045)*(1-t)*(1-t)
        particles[#particles+1]={position=vec(p.position[1]+p.velocity[1]*rise+wobble,
            p.position[2]+p.velocity[2]*rise,p.position[3]+p.velocity[3]*rise-wobble),
            normal=p.normal,radius=p.radius*(1+t*2.2),opacity=opacity,variation=p.variation}
    end
    ecs.emit('impact.visuals',{decals=decals,smoke=particles})
    had_visuals=#marks>0 or #smoke>0; changed=false
end

-- All controls edit the scene component and can be tuned in the ECS inspector.
for _,item in ipairs({
    {'impact_enabled','enabled','Impact effects'},
    {'impact_decals','decals_enabled','Bullet marks'},
    {'impact_smoke','smoke_enabled','Impact smoke'},
    {'impact_decal_size','decal_radius_m','Mark radius (m)',.02,.5},
    {'impact_decal_lifetime','decal_lifetime_s','Mark lifetime (s)',1,120},
    {'impact_smoke_size','smoke_radius_m','Smoke radius (m)',.02,1},
    {'impact_smoke_lifetime','smoke_lifetime_s','Smoke lifetime (s)',.1,5},
}) do
    local spec={component='ImpactEffects',field=item[2],label=item[3]}
    if item[4] then spec.min,spec.max=item[4],item[5] end
    engine.export(entity,item[1],spec)
end

ecs.on('projectile.impact',function(event)
    local cfg=settings()
    if cfg.enabled==false or event.kind~='level' or not vector(event.position) then return end
    local n=unit(event.normal); if not n then return end
    local rig=ids.player and ecs.get(ids.player,'PlayerRig') or nil
    local units=number(rig and rig.units_per_meter,1,.01,100)
    local state=ids.active_level and ecs.get(ids.active_level,'VoxelColliderState') or nil
    -- Voxel surfaces approximate splats. Offset slightly toward the camera-facing
    -- face to avoid embedding marks inside the occupied voxel or its splats.
    local resolution=number(state and state.resolution,0,0,.16*units)
    local offset=math.max(.018*units,resolution*1.25)
    local origin=vec(event.position[1]+n[1]*offset,event.position[2]+n[2]*offset,event.position[3]+n[3]*offset)
    local seed=number(event.shot,1,0,1000000)
    if cfg.decals_enabled~=false then
        if #marks>=MAX_MARKS then table.remove(marks,1) end
        marks[#marks+1]={position=origin,normal=n,age=0,
            life=number(cfg.decal_lifetime_s,30,1,120),
            radius=number(cfg.decal_radius_m,.09,.02,.5)*units,opacity=.95,variation=seed}
        changed=true
    end
    if cfg.smoke_enabled~=false then
        local life=number(cfg.smoke_lifetime_s,1.3,.1,5)
        local radius=number(cfg.smoke_radius_m,.09,.02,1)*units
        for i=1,5 do
            if #smoke>=MAX_SMOKE then table.remove(smoke,1) end
            local r1,r2,r3=random(seed*7+i),random(seed*13+i),random(seed*19+i)
            local speed=(.16+.22*r1)*units
            smoke[#smoke+1]={position=origin,normal=n,age=0,life=life*(.7+.3*r2),units=units,
                velocity=vec(n[1]*speed+(r2-.5)*.16*units,n[2]*speed+(.12+.18*r3)*units,
                    n[3]*speed+(r1-.5)*.16*units),
                radius=radius*(.72+.5*r3),opacity=.28,variation=seed+i*11}
        end
        changed=true
    end
end)

ecs.system('impact.effects',function(dt)
    dt=number(dt,0,0,.25)
    local cfg=settings()
    if cfg.enabled==false or cfg.decals_enabled==false then
        if #marks>0 then marks={}; changed=true end
    end
    if cfg.enabled==false or cfg.smoke_enabled==false then
        if #smoke>0 then smoke={}; changed=true end
    end
    for i=#marks,1,-1 do
        local mark=marks[i]; mark.age=mark.age+dt
        if mark.age>=mark.life then table.remove(marks,i); changed=true
        elseif mark.life-mark.age<=3 then changed=true end
    end
    for i=#smoke,1,-1 do
        local particle=smoke[i]; particle.age=particle.age+dt
        if particle.age>=particle.life then table.remove(smoke,i); changed=true end
    end
    publish()
end)
