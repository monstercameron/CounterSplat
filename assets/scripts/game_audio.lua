-- Gameplay chooses which scene emitters play; the host only mixes audio events.
local ids = assert(ecs.get_resource('scene.entities'))
local player = assert(ids.player)
local layer = ids.audio_root
local last_layer, layer_elapsed = nil, 0
local stride, elapsed = 0, 1
local function play(id, overrides)
    if id and ecs.is_alive(id) then engine.audio.play(id, overrides) end
end
local function sync_layer()
    if not layer or not ecs.is_alive(layer) then return end
    local value = ecs.get(layer, 'AudioLayer')
    if not value then return end
    local signature = table.concat({value.name, tostring(value.enabled), tostring(value.mix.gain),
        tostring(value.mix.muted), tostring(value.mix.solo)}, ':')
    if signature ~= last_layer then last_layer=signature; engine.audio.sync_layer(layer) end
end
sync_layer()
ecs.system('audio.layer_sync', function(dt)
    elapsed = elapsed + math.max(0, dt)
    layer_elapsed = layer_elapsed + math.max(0, dt)
    if layer_elapsed >= .25 then layer_elapsed=0; sync_layer() end
end)
ecs.on('weapon.shot', function(event)
    if event.player == player then play(ids.audio_shot) end
end)
ecs.on('weapon.reload_started', function(event)
    if event.entity == ids.first_person then play(ids.audio_reload) end
end)
ecs.on('weapon.dry_fire', function(event)
    if event.entity == ids.first_person then play(ids.audio_dry) end
end)
ecs.on('projectile.impact', function(event)
    if event.owner == player then play(ids.audio_impact) end
end)
ecs.on('physics.motion_resolved', function(event)
    if event.entity ~= player or type(event.moved) ~= 'table' then return end
    local x, z = event.moved[1], event.moved[3]
    if type(x) ~= 'number' or type(z) ~= 'number' or x ~= x or z ~= z or math.abs(x) > 10000000 or math.abs(z) > 10000000 then return end
    local body = ecs.get(player, 'PhysicsBody')
    if body and body.gravity_enabled and not body.grounded then stride=0; return end
    local rig = ecs.get(player, 'PlayerRig')
    local units = rig and rig.units_per_meter or 1
    if type(units) ~= 'number' or units ~= units or units < .01 or units > 100 then return end
    local distance = math.sqrt(x*x + z*z) / units
    if distance > 2 then stride=0; return end -- teleports are not footsteps
    stride = stride + distance
    if stride >= 1.5 and elapsed >= .18 then
        stride = stride % 1.5; elapsed = 0
        play(ids.audio_footstep)
    end
end)
ecs.on('input.reset', function() stride=0; elapsed=1 end)

-- Pause-menu edits use the same scene-owned audio layer as game audio.
ecs.on('settings.sound_request',function(request)
    if not layer or type(request)~='table' then return end
    local value=ecs.get(layer,'AudioLayer')
    if not value or type(value.mix)~='table' then return end
    if type(request.gain)=='number' and request.gain==request.gain then
        value.mix.gain=math.max(0,math.min(1,request.gain))
    elseif type(request.muted)=='boolean' then value.mix.muted=request.muted
    else return end
    ecs.set(layer,'AudioLayer',value)
    sync_layer()
end)
