-- Sustained-fire presentation is player-owned Lua behavior. Other weapons can
-- reuse it by emitting the standard weapon.shot event with the player owner.
local ids = assert(ecs.get_resource('scene.entities'))
local player = assert(ids.player)
local read_settings = engine.component_reader(player, 'CombatVision')
local defaults = {enabled=true, max_blur=.48, heat_per_shot=.12, threshold=.16,
    recovery_seconds=1.6, response_seconds=.22}
local entries = {
    {'combat_vision_enabled','enabled','Sustained fire vision'},
    {'combat_vision_strength','max_blur','Fire blur strength',0,1},
    {'combat_vision_per_shot','heat_per_shot','Fire blur buildup / shot',0,.5},
    {'combat_vision_threshold','threshold','Fire blur threshold',0,.9},
    {'combat_vision_recovery','recovery_seconds','Fire blur recovery (s)',.1,8},
    {'combat_vision_response','response_seconds','Fire blur response (s)',.05,2},
}
local function number(value, fallback, low, high)
    if type(value)~='number' or value~=value or math.abs(value)>1000000 then return fallback end
    return math.max(low,math.min(high,value))
end
local initial = ecs.get(player, 'CombatVision')
if initial then
    -- Allow older or partially authored components without re-enabling a toggle.
    local changed=false
    for _,entry in ipairs(entries) do
        local field,value=entry[2],initial[entry[2]]
        if entry[4] then value=number(value,defaults[field],entry[4],entry[5])
        elseif type(value)~='boolean' then value=defaults[field] end
        if initial[field]~=value then initial[field]=value; changed=true end
    end
    if changed then ecs.set(player,'CombatVision',initial) end
    for _,entry in ipairs(entries) do
        local spec={component='CombatVision',field=entry[2],label=entry[3]}
        if entry[4] then spec.min,spec.max=entry[4],entry[5] end
        engine.export(player,entry[1],spec)
    end
end
local heat, visual, published = 0, 0, nil
local function publish(value)
    if published==nil or math.abs(value-published)>=.001 or (value==0 and published~=0) then
        published=value
        ecs.set_resource('camera.combat_vision',value)
    end
end
local function reset() heat,visual=0,0; publish(0) end
publish(0)

ecs.on('weapon.shot',function(event)
    if type(event)~='table' or event.player~=player then return end
    local settings=read_settings()
    if not settings or settings.enabled~=true then return end
    heat=math.min(1,heat+number(settings.heat_per_shot,.12,0,.5))
end)
ecs.on('input.reset',reset)
ecs.on('combat.death',function(event)
    if type(event)=='table' and event.target==player then reset() end
end)

ecs.system('player.combat_vision',function(dt)
    dt=number(dt,0,0,.25)
    if dt==0 then return end
    local settings=read_settings()
    if not settings or settings.enabled~=true then
        if heat~=0 or visual~=0 then reset() end
        return
    end
    local recovery=number(settings.recovery_seconds,1.6,.1,8)
    local threshold=number(settings.threshold,.16,0,.9)
    local response=number(settings.response_seconds,.22,.05,2)
    heat=heat*math.exp(-dt/recovery)
    local buildup=math.max(0,math.min(1,(heat-threshold)/(1-threshold)))
    local target=number(settings.max_blur,.48,0,1)*buildup
    visual=visual+(target-visual)*(1-math.exp(-dt/response))
    if visual<.0005 and target==0 then visual=0 end
    publish(visual)
end)
