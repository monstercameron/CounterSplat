-- Translate weapon shots into the reusable scene-lighting event. The muzzle
-- offset and intensity remain ordinary, inspector-tunable ECS data.
local ids = assert(ecs.get_resource('scene.entities'))
local owner = assert(ids.first_person)
local read_settings = engine.component_reader(owner, 'MuzzleLight')

local function finite(v) return type(v)=='number' and v==v and math.abs(v)<=10000000 end
local function number(v, fallback, lo, hi)
    if not finite(v) then return fallback end
    return math.max(lo, math.min(hi, v))
end
local function vector(v)
    return type(v)=='table' and #v==3 and finite(v[1]) and finite(v[2]) and finite(v[3])
end
local function vec(x,y,z) local v=ecs.array(); v[1],v[2],v[3]=x,y,z; return v end

for _,item in ipairs({
    {'muzzle_light_enabled','enabled','Muzzle light'},
    {'muzzle_light_intensity','intensity','Muzzle light intensity',0,2},
    {'muzzle_light_radius','radius_m','Muzzle light radius (m)',.1,10},
    {'muzzle_light_duration','duration_s','Muzzle light duration (s)',.03,.3},
    {'muzzle_light_offset','offset_m','Muzzle light offset (m)',0,2},
    {'muzzle_light_red','red','Muzzle light red',0,1},
    {'muzzle_light_green','green','Muzzle light green',0,1},
    {'muzzle_light_blue','blue','Muzzle light blue',0,1},
}) do
    local spec={component='MuzzleLight',field=item[2],label=item[3]}
    if item[4] then spec.min,spec.max=item[4],item[5] end
    engine.export(owner,item[1],spec)
end

ecs.on('weapon.shot',function(event)
    local cfg=read_settings()
    if not cfg or cfg.enabled==false or type(event)~='table' or event.entity~=owner or
        not vector(event.origin) or not vector(event.direction) then return end
    local length=math.sqrt(event.direction[1]^2+event.direction[2]^2+event.direction[3]^2)
    if length<.00001 then return end
    local rig=ids.player and ecs.get(ids.player,'PlayerRig') or nil
    local units=number(rig and rig.units_per_meter,1,.01,100)
    local offset=number(cfg.offset_m,.65,0,2)*units/length
    engine.events.emit('lighting.flash',{
        source=owner, shot=event.shot,
        position=vec(event.origin[1]+event.direction[1]*offset,
                     event.origin[2]+event.direction[2]*offset,
                     event.origin[3]+event.direction[3]*offset),
        radius=number(cfg.radius_m,3,.1,10)*units,
        color=vec(number(cfg.red,1,0,1),number(cfg.green,.68,0,1),number(cfg.blue,.35,0,1)),
        intensity=number(cfg.intensity,.32,0,2),
        duration_s=number(cfg.duration_s,.09,.03,.3),
    })
end)
