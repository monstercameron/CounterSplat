-- Reusable, bounded transient-light service. Any Lua system may emit
-- lighting.flash with a world position, radius, RGB, intensity and lifetime.
-- The most recent flash owns the single local-light slot.
local active, sequence = nil, 0
local function finite(v) return type(v)=='number' and v==v and math.abs(v)<=10000000 end
local function number(v, fallback, lo, hi)
    if not finite(v) then return fallback end
    return math.max(lo, math.min(hi, v))
end
local function vector(v)
    return type(v)=='table' and #v==3 and finite(v[1]) and finite(v[2]) and finite(v[3])
end
local function vec(x,y,z) local v=ecs.array(); v[1],v[2],v[3]=x,y,z; return v end
local function clear()
    if active then active=nil; ecs.remove_resource('render.local_light') end
end
local function publish(strength)
    ecs.set_resource('render.local_light', {position=active.position, radius=active.radius,
        color=active.color, intensity=strength})
end

engine.events.on('lighting.flash',function(event)
    if type(event)~='table' or not vector(event.position) or not vector(event.color) or
       not finite(event.radius) or event.radius<=0 or not finite(event.intensity) or event.intensity<=0 or
       not finite(event.duration_s) or event.duration_s<=0 then return end
    sequence=sequence+1
    active={id=sequence, age=0, awaiting_present=true,
        position=vec(event.position[1],event.position[2],event.position[3]),
        color=vec(number(event.color[1],1,0,1),number(event.color[2],1,0,1),number(event.color[3],1,0,1)),
        radius=number(event.radius,1,.001,10000), intensity=number(event.intensity,0,0,2),
        duration=number(event.duration_s,.09,.01,2)}
    publish(active.intensity)
    -- A flash is held until one real canvas submission so short pulses cannot
    -- disappear when several fixed simulation ticks precede a rendered frame.
    ecs.emit('lighting.flash_started',{id=active.id})
end)
ecs.on('lighting.flash_presented',function(event)
    if active and type(event)=='table' and event.id==active.id then active.awaiting_present=false end
end)
ecs.on('input.reset',clear)
ecs.system('lighting.transient',function(dt)
    if not active or active.awaiting_present then return end
    active.age=active.age+number(dt,0,0,.25)
    if active.age>=active.duration then clear(); return end
    local remaining=1-active.age/active.duration
    publish(active.intensity*remaining*remaining)
end)
