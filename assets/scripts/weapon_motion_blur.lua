-- The viewmodel's motion blur follows actual mouse-driven camera rotation.
-- This script owns the response; the renderer receives only a bounded UV span.
local ids=assert(ecs.get_resource('scene.entities'))
local player,weapon=assert(ids.player),assert(ids.first_person)
local read_settings=engine.component_reader(weapon,'WeaponMotionBlur')
local read_controls=engine.component_reader(player,'PlayerControls')
local read_lens=engine.component_reader(player,'CameraLens')
local defaults={enabled=true,strength=.8,max_span=.012,response_seconds=.06,ads_scale=.5}
local entries={
    {'weapon_blur_enabled','enabled','Weapon motion blur'},
    {'weapon_blur_strength','strength','Weapon blur strength',0,2},
    {'weapon_blur_span','max_span','Weapon blur max span',0,.04},
    {'weapon_blur_response','response_seconds','Weapon blur response (s)',.02,.5},
    {'weapon_blur_ads','ads_scale','Weapon blur ADS scale',0,1},
}
local function number(value,fallback,low,high)
    if type(value)~='number' or value~=value or math.abs(value)>1000000 then return fallback end
    return math.max(low,math.min(high,value))
end
local initial=ecs.get(weapon,'WeaponMotionBlur')
if initial then
    local changed=false
    for _,entry in ipairs(entries) do
        local field,value=entry[2],initial[entry[2]]
        if entry[4] then value=number(value,defaults[field],entry[4],entry[5])
        elseif type(value)~='boolean' then value=defaults[field] end
        if initial[field]~=value then initial[field]=value; changed=true end
    end
    if changed then ecs.set(weapon,'WeaponMotionBlur',initial) end
    for _,entry in ipairs(entries) do
        local spec={component='WeaponMotionBlur',field=entry[2],label=entry[3]}
        if entry[4] then spec.min,spec.max=entry[4],entry[5] end
        engine.export(weapon,entry[1],spec)
    end
end
local previous=ecs.get(player,'PlayerCamera')
local previous_yaw,previous_pitch=previous and previous.yaw,previous and previous.pitch
local pending_x,pending_y,rate_x,rate_y=0,0,0,0
local published_x,published_y=nil,nil
local aiming=false
local function publish(x,y)
    if published_x==nil or math.abs(x-published_x)>.00002 or math.abs(y-published_y)>.00002 or
        (x==0 and y==0 and (published_x~=0 or published_y~=0)) then
        published_x,published_y=x,y
        ecs.set_resource('render.weapon_motion_blur',{x=x,y=y})
    end
end
local function refresh_baseline()
    local camera=ecs.get(player,'PlayerCamera')
    previous_yaw,previous_pitch=camera and camera.yaw,camera and camera.pitch
end
local function reset()
    pending_x,pending_y,rate_x,rate_y=0,0,0,0
    aiming=false
    refresh_baseline()
    publish(0,0)
end
publish(0,0)
ecs.on('input.reset',reset)
ecs.on('ui.action',function(event)
    if type(event)=='table' and event.id==ids.reset and event.kind and event.kind.type=='clicked' then reset() end
end)
ecs.on('weapon.aim',function(event) if type(event)=='table' then aiming=event.pressed==true end end)
ecs.on('input.mouse_button',function(event)
    if event.button=='right' and (event.gameplay or not event.pressed) then aiming=event.pressed==true end
end)
-- This listener runs after the camera controllers, so their clamped camera
-- rotation is measured rather than estimating it from raw mouse pixels.
ecs.on('input.mouse_delta',function(event)
    local camera=ecs.get(player,'PlayerCamera')
    local controls=read_controls()
    if not camera then previous_yaw,previous_pitch=nil,nil; return end
    if controls and controls.enabled and controls.mouse_look and previous_yaw and previous_pitch then
        local dx=(camera.yaw-previous_yaw+math.pi)%(2*math.pi)-math.pi
        local dy=previous_pitch-camera.pitch
        pending_x=number(pending_x+dx,0,-1,1)
        pending_y=number(pending_y+dy,0,-1,1)
    end
    previous_yaw,previous_pitch=camera.yaw,camera.pitch
end)

ecs.system('weapon.motion_blur',function(dt)
    dt=number(dt,0,0,.25)
    if dt==0 then return end
    local camera=ecs.get(player,'PlayerCamera')
    local controls,settings=read_controls(),read_settings()
    if not camera or not controls or not controls.enabled or not controls.mouse_look or
       not settings or settings.enabled~=true then
        pending_x,pending_y,rate_x,rate_y=0,0,0,0
        previous_yaw,previous_pitch=camera and camera.yaw,camera and camera.pitch
        publish(0,0)
        return
    end
    -- No lens lookup or resource writes are needed when the turn has settled.
    if pending_x==0 and pending_y==0 and rate_x==0 and rate_y==0 then
        previous_yaw,previous_pitch=camera.yaw,camera.pitch
        return
    end
    local lens=ecs.get_resource('camera.render_lens') or read_lens()
    if lens and lens.motion_blur==false then
        pending_x,pending_y,rate_x,rate_y=0,0,0,0
        previous_yaw,previous_pitch=camera.yaw,camera.pitch
        publish(0,0)
        return
    end
    -- Mouse updates can be batched ahead of several fixed ticks. Limit the
    -- inferred speed, then smooth it; a still camera naturally returns to zero.
    local sample_x=number(pending_x/dt,0,-8,8)
    local sample_y=number(pending_y/dt,0,-8,8)
    pending_x,pending_y=0,0
    local response=number(settings.response_seconds,.06,.02,.5)
    local blend=1-math.exp(-dt/response)
    rate_x=rate_x+(sample_x-rate_x)*blend
    rate_y=rate_y+(sample_y-rate_y)*blend
    local shutter_angle=number(lens and lens.shutter_angle_degrees,180,1,360)
    local shutter_fps=number(lens and lens.shutter_fps,24,1,240)
    local shutter=math.min(.05,shutter_angle/(360*shutter_fps))
    local gain=shutter*.20*number(settings.strength,.8,0,2)
    if aiming then gain=gain*number(settings.ads_scale,.5,0,1) end
    local x,y=rate_x*gain,rate_y*gain
    local max_span=number(settings.max_span,.012,0,.04)
    local magnitude=math.sqrt(x*x+y*y)
    if magnitude>max_span and magnitude>0 then x,y=x*max_span/magnitude,y*max_span/magnitude end
    if math.abs(x)<.00001 and math.abs(y)<.00001 then x,y,rate_x,rate_y=0,0,0,0 end
    publish(x,y)
    -- Follow recoil and scripted camera edits as the baseline without treating
    -- them as mouse turns on the next input event.
    previous_yaw,previous_pitch=camera.yaw,camera.pitch
end)
