-- Neural-Sight's weapon state machine, adapted to CounterSplat's ECS event API.
-- The renderer only receives clip/phase requests; all game decisions stay here.
local ids = assert(ecs.get_resource('scene.entities'))
local owner, player = assert(ids.first_person), assert(ids.player)
local read_controls = engine.component_reader(player, 'PlayerControls')
local read_rig = engine.component_reader(player, 'PlayerRig')
local read_weapon_config = engine.component_reader(owner, 'WeaponConfig')
local read_weapon_animation = engine.component_reader(owner, 'WeaponAnimation')
local read_weapon_view = engine.component_reader(owner, 'FirstPersonView')
local read_weapon_recoil = engine.component_reader(owner, 'WeaponRecoil')
local read_camera_controller = ids.camera_controller and engine.component_reader(ids.camera_controller, 'CameraController')
local clips = assert(ecs.get_resource('first_person_clips'))
local config = assert(ecs.get(owner,'WeaponConfig'))
-- Preserve older scenes while making the new timing fields available to the
-- inspector, whose exports require an existing numeric ECS field.
local added_timing=false
for name,default in pairs({sprint_fire_exit_seconds=.12,sprint_fire_raise_seconds=.14}) do
    if type(config[name])~='number' or config[name]~=config[name] then
        config[name]=default; added_timing=true
    end
end
if added_timing then ecs.set(owner,'WeaponConfig',config) end
local function setting(name, fallback, low, high)
    local n=config[name]
    if type(n)~='number' or n~=n then return fallback end
    return math.max(low,math.min(high,n))
end
local capacity=math.floor(setting('magazine_capacity',30,1,500))
local initial_ammo=math.floor(setting('initial_loaded',capacity,0,capacity))
-- Weapon profiles (countersplat_scene::weapon_profile) fill these from the gun's feed and fire modes:
-- `swap` reloads exchange a full magazine or speedloader, `top_up` reloads load loose rounds one at a time
-- from a reserve; `modes` lists the selectable fire modes in cycle order.
local reload_kind=config.reload_kind=='top_up' and 'top_up' or 'swap'
local initial_spares={}
if type(config.initial_spares)=='table' then
    for i=1,16 do
        local n=config.initial_spares[i]
        if n==nil then break end
        if type(n)~='number' or n~=n then n=capacity end
        initial_spares[i]=math.floor(math.max(0,math.min(reload_kind=='swap' and capacity or 9999,n)))
    end
else
    initial_spares={capacity,capacity,capacity}
end
local modes={}
if type(config.modes)=='table' then
    for i=1,3 do
        local m=config.modes[i]
        if m=='single' or m=='burst' or m=='auto' then modes[#modes+1]=m end
    end
end
if #modes==0 then modes={'single','burst','auto'} end
local function mode_allowed(mode)
    for _,m in ipairs(modes) do if m==mode then return true end end
    return false
end
local burst_rounds=math.floor(setting('burst_rounds',3,2,10))
for _, e in ipairs({
    {'fp_enabled','enabled','First-person view'},
    {'fp_opacity','opacity','View opacity',0,1},
    {'fp_key','key_threshold','Green key threshold',0,0.6},
    {'fp_softness','key_softness','Green key softness',0.01,0.8},
    {'fp_despill','despill','Green spill',0,0.2},
    {'fp_offset','vertical_offset','Vertical offset',-0.25,0.25},
}) do
    local spec={component='FirstPersonView',field=e[2],label=e[3]}
    if e[4] then spec.min,spec.max=e[4],e[5] end
    engine.export(owner,e[1],spec)
end
for _, e in ipairs({
    {'weapon_rate','fire_interval','Seconds per shot',0.03,5},
    {'weapon_reload','reload_seconds','Reload seconds',0.1,10},
    {'weapon_raise','hip_raise_seconds','Hip raise seconds',0.05,1},
    {'weapon_sprint_exit','sprint_fire_exit_seconds','Sprint-to-fire exit (sec)',0.05,0.5},
    {'weapon_sprint_raise','sprint_fire_raise_seconds','Sprint-to-fire raise (sec)',0.05,0.5},
    {'weapon_recovery','hip_recovery_seconds','Hip recovery seconds',0,3},
    {'weapon_repack','repack_round_seconds','Seconds per packed round',0.1,2},
}) do
    engine.export(owner,e[1],{component='WeaponConfig',field=e[2],label=e[3],min=e[4],max=e[5]})
end

-- Visual motion is configurable ECS data. The renderer receives only the
-- selected frames and transforms; all animation and weapon decisions stay here.
local animation=ecs.get(owner,'WeaponAnimation')
if animation then
    for _,e in ipairs({
        {'weapon_idle','idle_enabled','Animated idle'},
        {'weapon_idle_strength','idle_strength','Idle sway',0,3},
        {'weapon_idle_rate','idle_rate','Idle rate',0.2,3},
        {'weapon_gait','movement_strength','Weapon movement',0,3},
        {'weapon_recoil','recoil_strength','Weapon recoil',0,3},
        {'weapon_blend','clip_blend_seconds','Clip blend (sec)',0,0.3},
    }) do
        local spec={component='WeaponAnimation',field=e[2],label=e[3]}
        if e[4] then spec.min,spec.max=e[4],e[5] end
        engine.export(owner,e[1],spec)
    end
end
local function animation_setting(name,fallback,low,high)
    local n=animation and animation[name]
    if type(n)~='number' or n~=n then return fallback end
    return math.max(low,math.min(high,n))
end

-- Recoil changes the player's actual aim, independently of the filmed weapon.
-- Older scenes without WeaponRecoil retain their previous visual-only behavior.
local recoil=ecs.get(owner,'WeaponRecoil')
if recoil then
    for _,e in ipairs({
        {'camera_recoil_enabled','enabled','Camera recoil'},
        {'camera_recoil_rise','vertical_degrees','Muzzle rise per shot (deg)',0,3},
        {'camera_recoil_side','horizontal_degrees','Side kick per shot (deg)',0,2},
        {'camera_recoil_ads','ads_multiplier','ADS recoil multiplier',0,2},
        {'camera_recoil_hip','hip_multiplier','Hip recoil multiplier',0,2},
        {'camera_recoil_speed','rise_seconds','Recoil rise time (s)',.01,.25},
    }) do
        local spec={component='WeaponRecoil',field=e[2],label=e[3]}
        if e[4] then spec.min,spec.max=e[4],e[5] end
        engine.export(owner,e[1],spec)
    end
end
local function recoil_setting(name,fallback,low,high)
    local n=recoil and recoil[name]
    if type(n)~='number' or n~=n or math.abs(n)>1000000 then return fallback end
    return math.max(low,math.min(high,n))
end

-- The optional module preserves compatibility with scenes that do not load it.
local stability=engine.aim_stability and engine.aim_stability.new(owner,player) or nil

-- `start_stowed` spawns (and respawns) with the weapon put away, as if F had been
-- pressed: nothing is drawn until the player equips it. Scenes use it while the
-- weapon footage is unavailable; older scenes without it start ready.
local start_state=config.start_stowed==true and 'noGun' or 'lowReady'
local function new_weapon_state()
    return {
    state=start_state, mode=modes[1], ammo=initial_ammo, mags={table.unpack(initial_spares)}, shots=0,
    aim=false, held=false, pending=false, hip_shot=false, burst=0, cooldown=0, recovery=0,
    equip_requested=false,
    time=0, duration=0, target=nil, clip='raising', from=0, to=0,
    run_clock=0, flash=0, flash_fresh=false, flash_stance='hip', reload_slot=nil,
    reload_held=false, reload_time=0, repack_time=0, keys={},
    crouch=0, crouch_clock=0, crouch_moving=false, reaction=0, reaction_length=0,
    idle_clock=0, motion_clock=0, idle_settle=0, fire_pose_hold=0, pose_slope=0,
    motion_x=0, motion_y=0, motion_roll=0, look_x=0, look_y=0,
    gait_phase=0, gait_gain=0, last_travel=1, resolved_distance=0,
    recoil_x={x=0,v=0}, recoil_y={x=0,v=0}, recoil_roll={x=0,v=0},
    recoil_drift_x={x=0,v=0}, recoil_drift_y={x=0,v=0}, recoil_hip=false, recoil_heat=0, recoil_bias=0,
    camera_kick_pitch=0, camera_kick_yaw=0,
    }
end
local s = new_weapon_state()
-- Last selected source frame allows stow/death to lower any interrupted
-- stance, including crouch and sprint, without snapping to a different clip.
local last_pose=nil
-- Browser hosts report the texture and transform that were actually submitted.
-- Other hosts can use the most recent scene-authored weapon pose instead.
local presented_pose=nil
ecs.on('first_person.presentation',function(pose)
    presented_pose=type(pose)=='table' and pose.visible==true and pose or nil
end)
local HIP = 24/32
local function clamp(v,a,b) return math.max(a,math.min(b,v)) end
-- Return the current fade/lower amount so interruptions can continue from
-- the visible pose rather than snapping back to a fully raised weapon.
local function lowered_amount()
    if s.state=='noGun' or s.state=='dead' then return 1 end
    if s.state~='stow' and s.state~='equip' and s.state~='death' then return 0 end
    -- The holster footage lowers the weapon out of frame itself.
    if s.clip=='holster' then return 0 end
    local progress=clamp(s.time/math.max(s.duration,.001),0,1)
    local smooth=progress*progress*(3-2*progress)
    local start=s.lower_from
    if type(start)~='number' then start=s.state=='equip' and 1 or 0 end
    local target=s.state=='equip' and 0 or 1
    return start+(target-start)*smooth
end
local function reserve()
    local n=0
    for i=1,#s.mags do n=n+s.mags[i] end
    return n
end
-- Keep physical magazines: reloading swaps the fullest spare with the loaded one.
local function spare()
    local best=nil
    for i=1,#s.mags do if s.mags[i]>s.ammo and (not best or s.mags[i]>s.mags[best]) then best=i end end
    return best
end
-- A top-up reload loads loose rounds from the fullest reserve pouches until full.
local function top_up()
    local need=capacity-s.ammo
    while need>0 do
        local best=nil
        for i=1,#s.mags do if s.mags[i]>0 and (not best or s.mags[i]>s.mags[best]) then best=i end end
        if not best then break end
        local take=math.min(need,s.mags[best])
        s.mags[best]=s.mags[best]-take; s.ammo=s.ammo+take; need=need-take
    end
end
local function wants_shot() return s.hip_shot or s.pending or s.burst>0 or (s.mode=='auto' and s.held) end
local HIP_TRANSITIONS={hipRaise=true,hipLower=true,hipToADS=true,adsToHip=true}
-- Sprinting has one state per direction, each with its own looping clip. Carries into and out of a sprint
-- (runFromLow, runFromADS) end on whichever direction state the movement keys choose.
local RUN_CLIPS={runForward='runForward',runLeft='runLeft',runRight='runRight',runBackward='runBackward'}
local function is_run(state) return RUN_CLIPS[state]~=nil end
-- Inspect: `inspectEnter` turns the weapon into the observe pose, `inspecting` holds it, `inspectExit` returns
-- to low ready. The inspection HUD (Q) holds it while open; I holds it for `inspect_seconds` in all.
local INSPECT_STATES={inspectEnter=true,inspecting=true,inspectExit=true}
local function run_clip(state)
    local clip=RUN_CLIPS[state]
    return clip and clips[clip] and clip or 'runLateral'
end
local function phase()
    if s.duration<=0 then return s.to end
    local t=clamp(s.time/s.duration,0,1)
    if HIP_TRANSITIONS[s.state] then t=t*t*(3-2*t)+s.pose_slope*t*(1-t)*(1-t) end
    return s.from+(s.to-s.from)*t
end
local function phase_velocity()
    if s.duration<=0 or not HIP_TRANSITIONS[s.state] then return 0 end
    local t=clamp(s.time/s.duration,0,1)
    return (s.to-s.from)/s.duration*(6*t*(1-t)+s.pose_slope*(3*t*t-4*t+1))
end
local function transition(state,clip,from,to,duration,target)
    s.state,s.clip,s.from,s.to=state,clip,from,to
    s.time,s.duration,s.target=0,math.max(0.001,duration),target
    s.edge_a,s.edge_b,s.edge_duration=nil,nil,nil
    s.pose_slope=0
end
-- Retiming carries the current pose and same-direction momentum; reversals
-- start at the visible phase instead of cutting to a transition endpoint.
local function pose_transition(state,to,duration,target)
    local from,velocity=phase(),phase_velocity()
    transition(state,'raising',from,to,duration,target)
    if math.abs(to-from)>1e-8 then s.pose_slope=clamp(velocity*s.duration/(to-from),0,3) end
end
local function stable(state)
    -- Changing sprint direction keeps the running gait clock; starting a sprint restarts it.
    local was_running=is_run(s.state) and s.duration<=0
    s.state,s.duration,s.time,s.target=state,0,0,nil
    s.lower_from=nil
    if state=='ads' then s.clip,s.to='raising',1
    elseif state=='hipFire' then s.clip,s.to='raising',HIP
    elseif is_run(state) then s.clip,s.to=run_clip(state),0
    elseif state=='inspecting' then s.clip,s.to='inspect',.5
    elseif state=='slide' then s.clip,s.to=clips.slide and 'slide' or 'raising',0; s.slide_time=0
    else s.clip,s.to='raising',0 end
    s.from=s.to
    s.idle_settle=.12
    if is_run(state) and not was_running then s.run_clock=0 end
end
local function stop_trigger() s.held,s.pending,s.hip_shot,s.burst=false,false,false,0 end
local function reload()
    if s.equip_requested or s.state=='dead' or s.state=='death' or s.state=='noGun' or s.state=='repack' then return end
    local slot,seconds
    if reload_kind=='top_up' then
        local rounds=math.min(capacity-s.ammo,reserve())
        if rounds<=0 then return end
        slot,seconds='top_up',setting('reload_seconds',.5,.05,15)*rounds
    else
        slot,seconds=spare(),setting('reload_seconds',4,.1,15)
        if not slot then return end
    end
    if INSPECT_STATES[s.state] or s.state=='slideExit' then stable('lowReady') end
    -- Let a committed shot display its flash before changing clips.
    if s.flash>0 and (s.state=='ads' or s.state=='hipFire') then
        s.reload_requested=true; stop_trigger(); return
    end
    if s.duration>0 then
        if s.edge_a or HIP_TRANSITIONS[s.state] then s.reload_requested=true; stop_trigger() end
        return
    end
    stop_trigger(); s.reload_requested=false; s.recovery=0; s.reload_slot=slot
    local ads=s.state=='ads'
    transition(ads and 'reloadADS' or 'reload',ads and 'reloadADS' or 'reload',0,1,seconds,ads and 'ads' or 'lowReady')
    ecs.emit('weapon.reload_started', {entity=owner, aiming=ads})
end
-- Consolidation moves exactly one round at a time without creating ammunition.
local function pack_round()
    local rounds={s.ammo,table.unpack(s.mags)}
    local target,donor=nil,nil
    for i=1,#rounds do
        local n=rounds[i]
        if n>0 and n<capacity then
            if not target or n>rounds[target] then target=i end
            if not donor or n<rounds[donor] or (n==rounds[donor] and i>donor) then donor=i end
        end
    end
    if not target or not donor or target==donor then return false end
    rounds[target],rounds[donor]=rounds[target]+1,rounds[donor]-1
    s.ammo=rounds[1]
    for i=1,#s.mags do s.mags[i]=rounds[i+1] end
    return true
end
local function begin_repack()
    -- Only detachable magazines and speedloaders are consolidated; loose rounds have nothing to repack.
    if reload_kind~='swap' or s.duration>0 or s.state=='noGun' or s.state=='dead' or s.state=='death' then return end
    local partial=0
    for _,n in ipairs({s.ammo,table.unpack(s.mags)}) do if n>0 and n<capacity then partial=partial+1 end end
    if partial<2 then return end
    stop_trigger(); s.reload_requested=false; s.state='repack'; s.repack_time=0; s.clip='raising'; s.to=0
end
-- A shot is an ECS event; hit detection and audio can subscribe independently.
local function shoot(cadence_remainder)
    if s.cooldown>0 or s.duration>0 or (s.state~='ads' and s.state~='hipFire') then return end
    if s.ammo<=0 then s.pending=false; s.hip_shot=false; s.burst=0; s.held=false; ecs.emit('weapon.dry_fire',{entity=owner}); return end
    local interval=setting('fire_interval',.1,.03,5)
    s.cooldown=interval-(s.ammo>1 and clamp(cadence_remainder or 0,0,interval-.001) or 0)
    s.ammo=s.ammo-1; s.shots=s.shots+1
    s.flash=0.14; s.flash_fresh=true; s.flash_stance=s.state=='ads' and 'ads' or 'hip'
    if s.state=='hipFire' then
        s.recovery=setting('hip_recovery_seconds',1,0,3)
        s.fire_pose_hold=math.min(.2,(clips.fireHip and clips.fireHip.duration or .14))
    end
    s.idle_settle=.14
    -- Capture the rendered eye and aim at the instant this round leaves the weapon.
    -- A later camera movement or worker response must not redirect an existing shot.
    local camera=ecs.get(player,'PlayerCamera')
    local origin, direction=ecs.array(), ecs.array()
    local pitch=clamp(camera.pitch+(camera.view_bob_pitch or 0),-1.553343,1.553343)
    for i=1,3 do origin[i]=camera.position[i]+(camera.view_offset[i] or 0) end
    origin[2]=origin[2]+(camera.view_posture_offset_y or 0)
    -- Project the filmed sight through the same bottom-anchored transform as
    -- first_person.wgsl, so breathing, movement and prior recoil affect aim.
    -- These landmarks are measured on Neural-Sight's authored raising frames.
    local aspect=tonumber(ecs.get_resource('render.scene_aspect')) or 16/9
    if aspect~=aspect then aspect=16/9 end
    aspect=clamp(aspect,.2,5)
    local view=read_weapon_view() or {}
    local pose=presented_pose or ecs.get_resource('hud.weapon_pose') or {}
    local function finite(value,fallback)
        if type(value)~='number' or value~=value or math.abs(value)>1000000 then return fallback end
        return value
    end
    local default_u=s.flash_stance=='ads' and 640/1280 or 641/1280
    local default_v=s.flash_stance=='ads' and 361/720 or 329/720
    -- FirstPersonView.ads_sight is the front sight's pixel in the 1280x720 ADS frame (the fiber-optic dot on
    -- the post), so aimed shots leave through the sight the player lines up.
    local ads_sight=view.ads_sight
    if s.flash_stance=='ads' and type(ads_sight)=='table' and type(ads_sight[1])=='number' and type(ads_sight[2])=='number' then
        default_u,default_v=clamp(ads_sight[1]/1280,0,1),clamp(ads_sight[2]/720,0,1)
    end
    -- Optional per-frame source-image landmarks make filmed recoil follow the
    -- visible sight. An unannotated clip retains the old stance landmark.
    local function sight(clip,index)
        local data=type(clip)=='string' and clips[clip] or nil
        local anchors=type(data)=='table' and data.sight_anchors or nil
        local anchor=type(anchors)=='table' and type(index)=='number' and anchors[math.floor(index)] or nil
        if type(anchor)~='table' then return default_u,default_v end
        local size=type(data.sight_frame_size)=='table' and data.sight_frame_size or {}
        local width=finite(size[1],1280); local height=finite(size[2],720)
        if width<=0 or height<=0 then return default_u,default_v end
        return clamp(finite(anchor[1],default_u*width)/width,0,1),
            clamp(finite(anchor[2],default_v*height)/height,0,1)
    end
    local u,v=sight(pose.clip,pose.index)
    if type(pose.previous)=='table' then
        local a,b=sight(pose.previous.clip,pose.previous.index)
        local blend=clamp(finite(pose.blend,1),0,1)
        u,v=a+(u-a)*blend,b+(v-b)*blend
    end
    -- Match the renderer's bounds before projecting the presented sight.
    local motion_x=clamp(finite(pose.motion_x,s.motion_x),-.2,.2)
    local motion_y=clamp(finite(pose.motion_y,s.motion_y),-.5,.5)
    local motion_roll=clamp(finite(pose.motion_roll,s.motion_roll),-.2,.2)
    local vertical=clamp(finite(pose.vertical_offset,finite(view.vertical_offset,.055)),-.25,.25)
    local x,y=u-.5,v+vertical-1
    local c,r=math.cos(motion_roll),math.sin(motion_roll)
    local frame_x=c*x-r*y+.5+motion_x
    local frame_y=r*x+c*y+1+motion_y
    local source=16/9
    local screen_x,screen_y
    if aspect>source then screen_x,screen_y=.5+(frame_x-.5)*source/aspect,frame_y
    else screen_x,screen_y=frame_x,1+(frame_y-1)*aspect/source end
    -- Hip shots can aim through the camera centre (aim_from_camera); aimed shots always follow the sight.
    if view.enabled~=true or (view.aim_from_camera==true and s.flash_stance~='ads') or
       (type(view.opacity)=='number' and view.opacity<=0) then
        screen_x,screen_y=.5,.5
    end
    local fov=type(camera.view_fov_degrees)=='number' and camera.view_fov_degrees or camera.fov_degrees
    local tangent=math.tan(math.rad(clamp(fov,.01,178.999))*.5)
    local sx=(screen_x-.5)*2*aspect*tangent
    local sy=(.5-screen_y)*2*tangent
    local yaw,roll=camera.yaw,camera.roll+(camera.view_bob_roll or 0)+(camera.view_turn_roll or 0)
    local cy,ssy,cp,sp=math.cos(yaw),math.sin(yaw),math.cos(pitch),math.sin(pitch)
    local cr,sr=math.cos(roll),math.sin(roll)
    local forward={ssy*cp,sp,-cy*cp}
    local right={cy,0,ssy}
    local up={-ssy*sp,cp,cy*sp}
    local length=math.sqrt(1+sx*sx+sy*sy)
    for i=1,3 do
        direction[i]=(forward[i]+(right[i]*cr-up[i]*sr)*sx+(up[i]*cr+right[i]*sr)*sy)/length
    end
    ecs.emit('weapon.shot',{entity=owner,player=player,stance=s.flash_stance,shot=s.shots,
        origin=origin,direction=direction})
    -- A deterministic spring impulse supplies the original viewmodel recoil.
    -- The shot snapshot above is taken before this shot's own impulse.
    local noise=math.sin(s.shots*12.9898+78.233)*43758.5453
    local side=(noise-math.floor(noise))*2-1
    s.recoil_hip=s.flash_stance=='hip'
    s.recoil_heat=math.min(1,s.recoil_heat+.22)
    local memory=s.recoil_hip and .40 or .62
    s.recoil_bias=s.recoil_bias*memory+side*(1-memory)
    local stability_recoil=stability and stability:shot(s.mode) or 1
    recoil=read_weapon_recoil()
    if recoil and recoil.enabled~=false then
        local stance=s.recoil_hip and recoil_setting('hip_multiplier',1.1,0,2)
            or recoil_setting('ads_multiplier',.85,0,2)
        -- Queue a one-way camera displacement after capturing this round's ray.
        -- It is applied over subsequent ticks; nothing recenters the camera.
        local kick=stance*stability_recoil
        s.camera_kick_pitch=clamp(s.camera_kick_pitch+
            math.rad(recoil_setting('vertical_degrees',.55,0,3))*kick,0,math.rad(45))
        s.camera_kick_yaw=clamp(s.camera_kick_yaw+
            math.rad(recoil_setting('horizontal_degrees',.12,0,2))*side*kick,-math.rad(30),math.rad(30))
    end
    -- The view-model kick scales with the weapon's resolved recoil (WeaponRecoil.vertical_degrees, which its
    -- profile derives from felt recoil), relative to the 0.55-degree carbine these spring impulses were tuned on.
    local profile_kick=1
    if recoil and type(recoil.vertical_degrees)=='number' then profile_kick=clamp(recoil.vertical_degrees/.55,.4,4) end
    local weight=animation_setting('recoil_strength',1,0,3)*(s.recoil_hip and 1.15 or .8)*
        (1+s.recoil_heat*(s.recoil_hip and .3 or .15))*stability_recoil*profile_kick
    s.recoil_x.v=s.recoil_x.v+(side*.30+s.recoil_bias*.12)*weight*.64
    s.recoil_y.v=s.recoil_y.v-(.82+side*.13)*weight
    s.recoil_roll.v=s.recoil_roll.v+side*.24*weight*.64
    s.recoil_drift_x.v=s.recoil_drift_x.v+s.recoil_bias*.052*weight
    s.recoil_drift_y.v=s.recoil_drift_y.v-.05*weight
end
local function trigger(down)
    -- A quick hip tap is committed through a carry/raise. An ADS tap released
    -- before the gun reaches the aimed pose is cancelled, as in Neural-Sight.
    if not down then s.held=false; s.pending=false; return end
    if s.held or s.reload_requested or s.equip_requested or s.state=='reload' or s.state=='reloadADS' or s.state=='repack' or s.state=='stow' or s.state=='equip' or s.state=='noGun' or s.state=='dead' or s.state=='death' then return end
    s.held=true
    if s.mode=='burst' then s.burst=burst_rounds
    elseif s.mode=='single' then
        local ready=s.duration<=0 and s.cooldown<=0 and
            ((s.state=='ads' and s.aim) or (s.state=='hipFire' and not s.aim))
        if ready then shoot(0)
        elseif s.aim then s.pending=true
        else s.hip_shot=true end
    end
end
local function cycle_mode()
    local next_mode=modes[1]
    for i,m in ipairs(modes) do if m==s.mode then next_mode=modes[i%#modes+1] end end
    s.mode=next_mode
    stop_trigger()
end
local function equip()
    if s.state=='dead' or s.state=='death' or s.state=='stow' or s.state=='equip' then return end
    if s.flash>0 and (s.state=='ads' or s.state=='hipFire') then
        stop_trigger(); s.reload_requested=false; s.equip_requested=true
        s.reload_held=false; s.reload_time=0
        return
    end
    -- Stowing interrupts a carry or reload without completing a magazine swap.
    stop_trigger(); s.reload_requested=false; s.equip_requested=false
    s.reload_slot=nil; s.reload_held=false; s.reload_time=0
    -- From an idle low ready the holster footage carries the weapon to the hip and out of frame; equipping plays
    -- it in reverse. Any other pose (aiming, sprinting, mid-transition) freezes and lowers procedurally.
    local holster=clips.holster and (clips.holster.duration or .6)
    if s.state=='noGun' then
        s.frozen_pose=nil
        if holster then transition('equip','holster',1,0,holster,'lowReady')
        else transition('equip','raising',0,0,.7,'lowReady'); s.lower_from=1 end
    elseif holster and s.state=='lowReady' and s.duration<=0 then
        s.frozen_pose=nil; transition('stow','holster',0,1,holster,'noGun')
    else
        local current,lowered=phase(),lowered_amount()
        s.frozen_pose=last_pose
        -- Preserve the held aim input; it can resume after the weapon is equipped.
        transition('stow','raising',current,current,.5,'noGun'); s.lower_from=lowered
    end
end
-- Inspect from an idle low ready. `held` keeps the observe pose until end_inspect (the inspection HUD's
-- lifetime); otherwise it returns after `inspect_seconds`. Aim, fire, sprint or reload cancel it.
local INSPECT_TURN=.5
local function inspect(held)
    if s.state=='inspectEnter' or s.state=='inspecting' then
        if held then s.inspect_held=true end
        return
    end
    if s.state~='lowReady' or s.duration>0 or s.aim or s.flash>0 or s.reload_requested or s.equip_requested then return end
    stop_trigger()
    s.inspect_held=held==true
    s.inspect_left=math.max(0,setting('inspect_seconds',2.4,.5,10)-2*INSPECT_TURN)
    transition('inspectEnter','inspect',0,.5,INSPECT_TURN,'inspecting')
end
local function end_inspect()
    s.inspect_held=false
    if s.state=='inspecting' then transition('inspectExit','inspect',.5,1,INSPECT_TURN,'lowReady')
    elseif s.state=='inspectEnter' then
        -- The inspect clip is symmetric, so turning back early retraces the entry from its visible phase.
        local p=phase()
        transition('inspectExit','inspect',p,0,math.max(.01,INSPECT_TURN*p/.5),'lowReady')
    end
end
-- The player's crouch slide (player_controls.lua) drives the weapon's `slide` state.
ecs.on('player.slide',function(e)
    if type(e)=='table' and (e.player==nil or e.player==player) then s.sliding=e.active==true end
end)
-- External game scripts can use these ordinary ECS events without knowing the host.
ecs.on('weapon.inspect',function(e) inspect(type(e)=='table' and e.held==true) end)
-- The HUD may open while the weapon is busy (raised from a shot, aiming, sprinting); it then inspects as soon as
-- the weapon settles into an idle low ready, for as long as the HUD stays open.
engine.events.on('hud.inspect_started',function() s.inspect_wanted=true; inspect(true) end)
engine.events.on('hud.inspect_ended',function()
    s.inspect_wanted=false
    if s.inspect_held then end_inspect() end
end)
ecs.on('weapon.trigger',function(e) trigger(e.pressed==true) end)
ecs.on('weapon.aim',function(e) s.aim=e.pressed==true end)
ecs.on('weapon.reload',reload)
ecs.on('weapon.equip',equip)
ecs.on('weapon.mode',cycle_mode)
ecs.on('weapon.hit',function(e)
    if type(e)~='table' or (e.target~=nil and e.target~=player and e.target~=owner) or
       (e.entity~=nil and e.entity~=player and e.entity~=owner) then return end
    if s.state~='dead' then
        s.reaction_length=e.near==true and .16 or .35
        s.reaction=s.reaction_length
        s.idle_settle=s.reaction_length+.12
        if stability then stability:under_fire(e.near==true and .55 or 1,e.near~=true) end
    end
end)
-- Future gameplay scripts may report a near miss or explicit suppression with
-- {target=player, intensity=0..1, owner=attacker}; damage uses the same model.
for _,channel in ipairs({'combat.near_miss','combat.suppression'}) do
    ecs.on(channel,function(e)
        if stability and type(e)=='table' and e.target==player and e.owner~=player then
            stability:under_fire(e.intensity,false)
        end
    end)
end
ecs.on('combat.damage',function(e)
    if stability and type(e)=='table' and e.target==player and type(e.amount)=='number' and e.amount>0 then
        stability:under_fire(clamp(e.amount/35,.2,1),true)
    end
end)
ecs.on('weapon.die',function()
    if s.state=='dead' or s.state=='death' then return end
    local stowed=s.state=='noGun'
    local current,lowered=phase(),lowered_amount()
    s.frozen_pose=last_pose; stop_trigger(); s.aim=false
    s.reload_requested=false; s.equip_requested=false; s.reload_slot=nil; s.reload_held=false; s.reload_time=0
    if stowed then stable('dead')
    else transition('death','raising',current,current,.8,'dead'); s.lower_from=lowered end
end)
-- A physical posture system can supply its collision-checked crouch amount.
-- This changes footage only; it never moves or resizes the player collider.
ecs.on('weapon.posture',function(e)
    if type(e.crouch)=='number' and e.crouch==e.crouch then
        local next=clamp(e.crouch,0,1)
        if next~=s.crouch then s.crouch_direction=next>s.crouch and 1 or -1 end
        s.crouch=next
    end
    s.crouch_moving=e.moving==true
end)
ecs.on('input.mouse_button',function(e)
    if e.button=='right' and (e.gameplay or not e.pressed) then s.aim=e.pressed==true end
    if e.button=='left' and (e.gameplay or not e.pressed) then trigger(e.pressed==true) end
end)
-- Camera look remains immediate; only the screen-space weapon lags and settles.
ecs.on('input.mouse_delta',function(event)
    local controls=read_controls()
    if not controls or not controls.enabled or not controls.mouse_look then return end
    local dx,dy=tonumber(event.dx) or 0,tonumber(event.dy) or 0
    if dx~=dx or dy~=dy then return end
    local sensitivity=controls.mouse_sensitivity or .002
    s.look_x=clamp(s.look_x-clamp(dx,-2000,2000)*sensitivity*.35,-.035,.035)
    s.look_y=clamp(s.look_y-clamp(dy,-2000,2000)*sensitivity*.25,-.025,.025)
end)
ecs.on('physics.motion_resolved',function(event)
    if event.entity~=player or type(event.moved)~='table' then return end
    local x,z=tonumber(event.moved[1]),tonumber(event.moved[3])
    if x and z and x==x and z==z then s.resolved_distance=s.resolved_distance+math.sqrt(x*x+z*z) end
end)
ecs.on('input.reset',function()
    stop_trigger(); s.aim=false; s.keys={}; s.reload_held=false; s.reload_time=0
    s.reload_requested=false; s.equip_requested=false; s.sliding=false; s.inspect_wanted=false
    s.look_x,s.look_y,s.resolved_distance=0,0,0
    s.camera_kick_pitch,s.camera_kick_yaw=0,0
    presented_pose=nil
    if s.state=='repack' then stable('lowReady') end
end)
ecs.on('input.key',function(e)
    if type(e.code)~='string' then return end
    if e.pressed and not e.gameplay then return end
    s.keys[e.code]=e.pressed==true or nil
    if e.code=='KeyR' then
        if e.pressed and not e.is_repeat then s.reload_held=true; s.reload_time=0
        elseif not e.pressed and s.reload_held then
            s.reload_held=false
            if s.state=='repack' then stable('lowReady') elseif s.reload_time<.65 then reload() end
        end
    elseif e.pressed and not e.is_repeat then
        if e.code=='KeyV' then cycle_mode() elseif e.code=='KeyF' then equip() elseif e.code=='KeyI' then inspect(false) end
    end
end)

local last_frame,last_state,last_clip=nil,nil,nil
local ammo_pose_signature=nil
-- Reusable reset event restores the weapon after a mode restart or defeat.
ecs.on('weapon.reset',function(event)
    if type(event)=='table' and event.entity~=nil and event.entity~=owner then return end
    local mode=s.mode
    s=new_weapon_state()
    if mode_allowed(mode) then s.mode=mode end
    last_pose,presented_pose,last_frame,last_state,last_clip,ammo_pose_signature=nil,nil,nil,nil,nil,nil
    if stability and stability.reset then stability:reset() end
end)
-- Clips that share endpoint frames, so switching between them needs no crossfade. runForward is the carries'
-- run endpoint; the other directions crossfade in.
local canonical={raising=true,runFromLow=true,runFromADS=true,runLateral=true,runForward=true}
local function is_fire(clip) return clip=='fireHip' or clip=='fireADS' end
-- Select authored loops, shared carry bridges and interrupted crouch phases.
-- The host streams these frames and blends only discontinuous clip handoffs.
local function present()
    local view=read_weapon_view()
    if not view then s.flash_fresh=false; return end
    local visible=view.enabled==true and s.state~='noGun' and s.state~='dead'
    local clip,p,direction,loop=s.clip,phase(),1,false
    if s.edge_a then direction=s.to>=s.from and 1 or -1
    elseif HIP_TRANSITIONS[s.state] or s.clip=='holster' then direction=s.to>=s.from and 1 or -1 end
    local fire=s.flash_stance=='ads' and 'fireADS' or 'fireHip'
    if (s.state=='stow' or s.state=='death') and s.frozen_pose then
        clip,p,direction=s.frozen_pose.clip,s.frozen_pose.p,s.frozen_pose.direction
    elseif s.flash>0 and (s.state=='ads' or s.state=='hipFire') and clips[fire] and clips[fire].enabled~=false then
        clip=fire; p=1-s.flash/(clips[fire].duration or .14)
    elseif is_run(s.state) then
        clip=run_clip(s.state)
        local period=clips[clip].duration or 1.6
        p=(s.run_clock%period)/period; loop=true
    elseif s.state=='slide' and clip=='slide' then
        -- The drop into the slide plays over the clip's duration, then holds until the slide ends.
        p=clamp((s.slide_time or 0)/math.max(.01,clips.slide.duration or .4),0,1)
    elseif s.crouch>0 and (s.state=='lowReady' or s.state=='ads') and s.duration<=0 then
        local stance=s.state=='ads' and 'ADS' or 'Low'
        if s.crouch<.999 then
            -- Entry and derived exit share the same source frames, so a
            -- reversal resumes the exact visible physical crouch amount.
            clip='crouchEnter'..stance; p=s.crouch; direction=s.crouch_direction or 1
        else
            clip='crouch'..(s.crouch_moving and 'Walk' or 'Idle')..stance
            p=(s.crouch_clock%clips[clip].duration)/clips[clip].duration; loop=true
        end
    elseif s.state=='lowReady' and s.duration<=0 and animation and animation.idle_enabled~=false and clips.lowReady then
        clip='lowReady'; p=(s.idle_clock%clips.lowReady.duration)/clips.lowReady.duration; loop=true
    end
    if not clips[clip] then clip,p,direction,loop='raising',0,1,false end
    -- A shot accepted between ticks must reach the first flash frame before
    -- elapsed simulation time can advance its animation.
    s.flash_fresh=false
    last_pose={clip=clip,p=p,direction=direction}
    local frame=math.floor(clamp(p,0,1)*(clips[clip].frames-1)+.5)+1
    local blend=0
    if last_clip and last_clip~=clip and not is_fire(last_clip) and not is_fire(clip) and
        not (canonical[last_clip] and canonical[clip]) then
        blend=animation_setting('clip_blend_seconds',.12,0,.3)
    end
    local reaction_offset=0
    if not stability and s.reaction>0 then
        local elapsed=s.reaction_length-s.reaction
        reaction_offset=.008*(s.reaction/s.reaction_length)*math.sin(elapsed*32)
    end
    local action_offset,opacity=0,view.opacity
    if s.state=='stow' or s.state=='equip' or s.state=='death' then
        local lowered=lowered_amount()
        action_offset=.40*lowered; opacity=opacity*(1-lowered)
    end
    local mx,my,mr=s.motion_x,s.motion_y+reaction_offset+action_offset,s.motion_roll
    local signature=table.concat({clip,frame,tostring(visible),opacity,view.key_threshold,view.key_softness,view.despill,
        view.vertical_offset,('%0.5f'):format(mx),('%0.5f'):format(my),('%0.5f'):format(mr),direction,tostring(loop)},':')
    if signature~=last_frame then
        last_frame=signature
        ecs.emit('first_person.frame',{clip=clip,index=frame,visible=visible,opacity=opacity,
            key_threshold=view.key_threshold,key_softness=view.key_softness,despill=view.despill,vertical_offset=view.vertical_offset,
            motion_x=mx,motion_y=my,motion_roll=mr,direction=direction,loop=loop,transition_seconds=blend,
            loop_blend_seconds=animation_setting('clip_blend_seconds',.12,0,.3)})
        last_clip=clip
    end
    -- Publish changed poses for the persistent ammo callout. Browser hosts
    -- override these with the frame actually installed by the compositor.
    if signature~=ammo_pose_signature then
        ammo_pose_signature=signature
        ecs.set_resource('hud.weapon_pose',{clip=clip,index=frame,visible=visible,opacity=opacity,
            vertical_offset=view.vertical_offset,motion_x=mx,motion_y=my,motion_roll=mr})
    end
    local status=table.concat({s.state,s.mode,s.ammo,reserve(),s.shots,s.reaction,clip},':')
    if status~=last_state then
        last_state=status
        local magazines=ecs.array(); for i=1,#s.mags do magazines[i]=s.mags[i] end
        ecs.set(owner,'WeaponState',{state=s.state,mode=s.mode,ammo=s.ammo,reserve=reserve(),magazines=magazines,
            shots=s.shots,reaction=s.reaction,clip=clip})
    end
end
local function is_down(bindings,name)
    local code=bindings and bindings[name]
    return engine.input.is_down(s.keys,code)
end
-- Movement keys, whether the player sprints, and which run direction state a sprint uses: any forward input
-- runs forward, any backward input runs backward, a pure strafe runs left or right.
local function movement(c)
    if not c or not c.enabled then return false,false,'runForward' end
    local b=c.bindings
    local forward=(is_down(b,'forward') and 1 or 0)-(is_down(b,'backward') and 1 or 0)
    local side=(is_down(b,'right') and 1 or 0)-(is_down(b,'left') and 1 or 0)
    local moving=forward~=0 or side~=0
    local direction=forward>0 and 'runForward' or forward<0 and 'runBackward' or
        side<0 and 'runLeft' or side>0 and 'runRight' or 'runForward'
    -- Locomotion stays at sprint speed during hip fire; the weapon's carry
    -- target is selected separately below. ADS and crouch still block sprint.
    return moving,moving and is_down(b,'sprint') and s.crouch<.001 and not s.aim,direction
end
local previous_position=nil
local function spring(axis,dt,frequency,damping)
    local decay=math.exp(-damping*frequency*dt)
    local omega=frequency*math.sqrt(1-damping*damping)
    local c,si=math.cos(omega*dt),math.sin(omega*dt)
    local x,v,f=axis.x,axis.v,damping*frequency
    axis.x=decay*(x*c+(v+f*x)*si/omega)
    axis.v=decay*(v*c-(f*v+frequency*frequency*x)*si/omega)
end
-- Distance comes from collision-resolved movement, so walking into a wall
-- settles the viewmodel. With collision disabled, use the actual camera delta.
local function update_motion(dt,moving,running,controls)
    local camera=ecs.get(player,'PlayerCamera')
    local rig=read_rig()
    local body=ecs.get(player,'PhysicsBody')
    if not camera or not controls or not rig then return end
    local captured=ecs.get_resource('voxel.capture_movement')==true and body and body.enabled and body.collision_enabled
    local distance=0
    if captured then distance=s.resolved_distance
    elseif previous_position then
        local dx,dz=camera.position[1]-previous_position[1],camera.position[3]-previous_position[3]
        distance=math.sqrt(dx*dx+dz*dz)
        local limit=controls.move_speed*controls.sprint_multiplier*rig.units_per_meter*dt*2+.25
        if distance>limit then distance=0 end
    end
    s.resolved_distance=0
    previous_position={camera.position[1],camera.position[2],camera.position[3]}
    local grounded=not captured or not body.gravity_enabled or body.grounded
    if distance>.00001 and moving and grounded then
        s.last_travel=0
        s.gait_phase=(s.gait_phase+math.pi*distance/(rig.units_per_meter*(running and 1.4 or .85)))%(2*math.pi)
    else s.last_travel=s.last_travel+dt end
    local active=moving and grounded and s.last_travel<.12
    local gain=active and 1 or 0
    if s.crouch>.001 then gain=gain*.45 end
    if s.aim then gain=gain*.25 end
    local ease=1-math.exp(-dt*12)
    s.gait_gain=s.gait_gain+(gain-s.gait_gain)*ease
    local carry=0
    if is_run(s.edge_b) then carry=clamp(phase(),0,1)
    elseif is_run(s.state) then carry=1 end
    local wave=math.sin(s.gait_phase)
    local gait=animation_setting('movement_strength',1,0,3)*s.gait_gain
    local gait_x=wave*(.0035*(1-carry)+.045*carry)*gait
    local gait_y=math.cos(s.gait_phase*2)*.004*(1-carry)*gait
    local gait_roll=wave*.045*carry*gait
    s.look_x=s.look_x*math.exp(-dt*10); s.look_y=s.look_y*math.exp(-dt*10)
    local aim_weight=s.aim and .25 or 1
    local idle=animation and animation.idle_enabled~=false and animation_setting('idle_strength',1,0,3) or 0
    local clock=s.motion_clock*2*math.pi*animation_setting('idle_rate',1,.2,3)
    local idle_x=(.0025*math.sin(clock*.19)+.0008*math.sin(clock*.47+.7))*idle*aim_weight
    local idle_y=(.0028*math.sin(clock*.26+.9)+.0005*math.sin(clock*.41+1.7))*idle*aim_weight
    local idle_roll=.0015*math.sin(clock*.19+.4)*idle*aim_weight
    local hip=s.recoil_hip
    spring(s.recoil_x,dt,18,hip and .78 or .86)
    spring(s.recoil_y,dt,38,hip and .60 or .68)
    spring(s.recoil_roll,dt,16,hip and .82 or .90)
    spring(s.recoil_drift_x,dt,8,.95)
    spring(s.recoil_drift_y,dt,10,.95)
    s.recoil_heat=s.recoil_heat*math.exp(-dt*2.2)
    local stability_x,stability_y=0,0
    if stability then
        local units=math.max(.01,rig.units_per_meter)
        local walk_speed=math.max(.01,controls.move_speed)
        local speed=grounded and distance/(math.max(dt,.00001)*units) or 0
        local angle_x,angle_y=stability:update(dt,speed/walk_speed,s.crouch,s.aim)
        -- The source footage is bottom-anchored and may be aspect-cropped.
        -- Convert angular sway to the same normalized source-frame offsets
        -- consumed by both the compositor and shoot(), including zoom.
        local aspect=tonumber(ecs.get_resource('render.scene_aspect')) or 16/9
        if aspect~=aspect then aspect=16/9 end
        aspect=clamp(aspect,.2,5)
        local fov=type(camera.view_fov_degrees)=='number' and camera.view_fov_degrees or camera.fov_degrees
        local tangent=math.tan(math.rad(clamp(fov,.01,178.999))*.5)
        stability_x=math.tan(math.rad(angle_x))/(2*aspect*tangent)
        stability_y=-math.tan(math.rad(angle_y))/(2*tangent)
        if aspect>16/9 then stability_x=stability_x*aspect/(16/9)
        else stability_y=stability_y*(16/9)/aspect end
    end
    s.motion_x=clamp(idle_x+gait_x+s.look_x*aim_weight+s.recoil_x.x+s.recoil_drift_x.x+stability_x,-.10,.10)
    s.motion_y=clamp(idle_y+gait_y+s.look_y*aim_weight+s.recoil_y.x+s.recoil_drift_y.x+stability_y,-.10,.10)
    s.motion_roll=clamp(idle_roll+gait_roll+s.recoil_roll.x,-.10,.10)
end

-- Each carry clip joins shared endpoint images. Reversing a transition starts
-- at its current phase, including when aim or sprint changes mid-animation.
-- Each carry is its own named transition state between its two endpoint poses.
local function carry_state(from,to)
    if is_run(to) then return from=='ads' and 'adsToRun' or 'lowToRun' end
    if to=='ads' then return is_run(from) and 'runToADS' or 'raising' end
    return is_run(from) and 'runToLow' or 'lowering'
end
-- The run end of a carry is whichever direction state the sprint uses.
local function edge(a,b)
    local run=is_run(a) and a or is_run(b) and b or nil
    if (a=='lowReady' and b=='ads') or (b=='lowReady' and a=='ads') then return 'raising','lowReady','ads',.28 end
    if run and (a=='lowReady' or b=='lowReady') then return 'runFromLow','lowReady',run,.24 end
    if run and (a=='ads' or b=='ads') then return 'runFromADS','ads',run,.28 end
end
local function carry(target,quick_fire)
    local clip,a,b,duration=edge(s.state,target)
    if not clip then return false end
    if quick_fire and is_run(s.state) and target=='lowReady' then
        duration=math.min(duration,setting('sprint_fire_exit_seconds',.12,.05,.5))
    end
    local from=s.state==a and 0 or 1
    local to=target==a and 0 or 1
    transition(carry_state(s.state,target),clip,from,to,duration,target)
    s.edge_a,s.edge_b,s.edge_duration=a,b,duration
    return true
end
local function reverse_carry(target,quick_fire)
    if not s.edge_a then return false end
    -- A direction change while carrying into a sprint lands in the new direction.
    if is_run(target) and is_run(s.edge_b) and s.edge_b~=target then
        s.edge_b=target
        if s.to==1 then s.target=target end
    end
    local desired=target==s.edge_a and 0 or target==s.edge_b and 1 or (phase()<.5 and 0 or 1)
    if desired==s.to then return false end
    local p=phase(); local a,b,clip,duration=s.edge_a,s.edge_b,s.clip,s.edge_duration
    local end_state=desired==0 and a or b
    if quick_fire and (is_run(a) or is_run(b)) and not is_run(end_state) then
        duration=math.min(duration,setting('sprint_fire_exit_seconds',.12,.05,.5))
    end
    transition(carry_state(desired==0 and b or a,end_state),clip,p,desired,math.max(.01,math.abs(desired-p)*duration),end_state)
    s.edge_a,s.edge_b,s.edge_duration=a,b,duration
    return true
end

-- Apply each shot's pending kick smoothly to the live camera. This is additive,
-- so mouse corrections made during the rise are preserved instead of overwritten.
local function update_camera_recoil(dt)
    if s.camera_kick_pitch==0 and s.camera_kick_yaw==0 then return end
    recoil=read_weapon_recoil()
    if not recoil or recoil.enabled==false then
        s.camera_kick_pitch,s.camera_kick_yaw=0,0
        return
    end
    local camera=ecs.get(player,'PlayerCamera')
    if not camera then
        s.camera_kick_pitch,s.camera_kick_yaw=0,0
        return
    end
    local fraction=1-math.exp(-dt/recoil_setting('rise_seconds',.045,.01,.25))
    local pitch_delta=s.camera_kick_pitch*fraction
    local yaw_delta=s.camera_kick_yaw*fraction
    s.camera_kick_pitch=s.camera_kick_pitch-pitch_delta
    s.camera_kick_yaw=s.camera_kick_yaw-yaw_delta
    if math.abs(s.camera_kick_pitch)<.000001 then s.camera_kick_pitch=0 end
    if math.abs(s.camera_kick_yaw)<.000001 then s.camera_kick_yaw=0 end
    local controller=read_camera_controller and read_camera_controller()
    local degrees=controller and tonumber(controller.pitch_limit_degrees) or 89
    if not degrees or degrees~=degrees then degrees=89 end
    local limit=math.min(1.553343,math.rad(clamp(degrees,1,89)))
    local yaw=(camera.yaw+yaw_delta+math.pi)%(2*math.pi)-math.pi
    -- If a scene starts beyond the current look limit, recoil must never snap
    -- the camera downward; let the next mouse movement bring it into range.
    local pitch=math.max(camera.pitch,math.min(limit,camera.pitch+pitch_delta))
    ecs.set_fields(player,'PlayerCamera',{pitch=pitch,yaw=yaw})
end

-- Fixed-tick simulation: settle actions, resolve carry, then commit shots.
ecs.system('player.first_person',function(dt)
    dt=clamp(dt,0,.1)
    config=read_weapon_config() or config
    animation=read_weapon_animation() or animation
    if dt<=0 then present(); return end
    update_camera_recoil(dt)
    local controls=read_controls()
    local moving,sprint,run_direction=movement(controls)
    -- Hold the authored idle briefly after an action; breathing has its own
    -- clock and remains alive while the filmed transition settles.
    s.motion_clock=s.motion_clock+dt
    if s.idle_settle<=0 or moving or s.duration>0 then s.idle_clock=s.idle_clock+dt end
    s.idle_settle=math.max(0,s.idle_settle-dt)
    s.fire_pose_hold=math.max(0,s.fire_pose_hold-dt)
    update_motion(dt,moving,sprint,controls)
    local uninterrupted=(s.mode=='auto' and s.held) or (s.mode=='burst' and s.burst>0)
    local cadence_remainder=s.cooldown>0 and s.duration<=0 and
        (s.state=='ads' or s.state=='hipFire') and uninterrupted and math.max(0,dt-s.cooldown) or 0
    s.cooldown=math.max(0,s.cooldown-dt); if s.cooldown<1e-8 then s.cooldown=0 end
    s.crouch_clock=s.crouch_clock+dt
    s.recovery=math.max(0,s.recovery-dt)
    if not s.flash_fresh then s.flash=math.max(0,s.flash-dt) end
    s.reaction=math.max(0,s.reaction-dt)
    if s.reload_held then
        s.reload_time=s.reload_time+dt
        if s.reload_time>=.65 and s.state~='repack' then begin_repack() end
    end
    if s.state=='repack' then
        s.repack_time=s.repack_time+dt
        local interval=setting('repack_round_seconds',.4,.1,2)
        while s.repack_time+1e-9>=interval do s.repack_time=math.max(0,s.repack_time-interval); if not pack_round() then stable('lowReady'); break end end
        present(); return
    end
    -- Stow takes priority over a queued reload, but never removes a flash
    -- before the short firing clip has had a chance to play.
    if s.equip_requested then
        if s.flash<=0 then s.equip_requested=false; equip() end
        present(); return
    end
    if s.reload_requested and s.duration<=0 then
        if s.flash<=0 then s.reload_requested=false; reload() end
        present(); return
    end
    local shooting=wants_shot()
    -- A slide takes over from low ready, a sprint or a carry between them; aiming or firing leaves it while the
    -- body keeps sliding, and when the slide ends the weapon settles into the crouch.
    local slide_from=s.state=='lowReady' or is_run(s.state) or s.state=='lowToRun' or s.state=='runToLow' or
        s.state=='slideExit'
    if s.sliding and not s.aim and not shooting and (s.state=='slide' or slide_from) then
        if s.state~='slide' then s.edge_a,s.edge_b,s.edge_duration=nil,nil,nil; stable('slide') end
        s.slide_time=(s.slide_time or 0)+dt
        present(); return
    end
    if s.state=='slide' then
        -- Getting up plays the slide exit into low ready; aiming or firing skips straight to the weapon.
        if clips.slideExit and not (s.aim or shooting) then
            transition('slideExit','slideExit',0,1,clips.slideExit.duration or .45,'lowReady')
        else stable('lowReady') end
    end
    if s.state=='slideExit' and (s.aim or shooting) then stable('lowReady') end
    local quick_fire=sprint and not s.aim and shooting
    -- A raised hip-fire pose can coexist with sprint locomotion. Only return
    -- to the run carry after the requested shot/burst and flash have finished.
    local target=s.aim and 'ads' or (sprint and not shooting) and run_direction or 'lowReady'
    if sprint and not shooting and s.fire_pose_hold<=0 and (s.state=='hipFire' or
        (s.state=='hipLower' and s.duration-s.time>.16)) then
        s.recovery=0; pose_transition('hipLower',0,.16,'lowReady'); present(); return
    end
    if s.edge_a and reverse_carry(target,quick_fire) then present(); return end
    if INSPECT_STATES[s.state] and (s.aim or shooting or sprint) then s.inspect_held=false; stable('lowReady') end
    if s.inspect_wanted and s.state=='lowReady' and s.duration<=0 and not (s.aim or shooting or sprint) then
        inspect(true)
    end
    if s.state=='inspecting' and not s.inspect_held then
        s.inspect_left=(s.inspect_left or 0)-dt
        if s.inspect_left<=0 then end_inspect() end
    end
    if s.duration>0 then
        -- Hip raises/lowers are reversible from their current filmed phase.
        if s.state=='hipLower' and not s.aim and shooting then
            local remaining=math.abs(HIP-phase())/HIP
            local raise=quick_fire and setting('sprint_fire_raise_seconds',.14,.05,.5)
                or setting('hip_raise_seconds',.22,.05,1)
            pose_transition('hipRaise',HIP,math.max(.03,remaining*raise),'hipFire')
        elseif (s.state=='hipRaise' or s.state=='hipLower' or s.state=='hipToADS' or s.state=='adsToHip') and s.aim and s.to~=1 then
            pose_transition('hipToADS',1,.18,'ads')
        elseif s.state=='hipToADS' and not s.aim then pose_transition('adsToHip',HIP,.18,'hipFire') end
        s.time=s.time+dt
        if s.time+1e-9>=s.duration then
            if (s.state=='reload' or s.state=='reloadADS') and s.reload_slot then
                if s.reload_slot=='top_up' then top_up()
                else s.ammo,s.mags[s.reload_slot]=s.mags[s.reload_slot],s.ammo end
                s.reload_slot=nil
            end
            local done=s.target; s.edge_a,s.edge_b,s.edge_duration=nil,nil,nil; stable(done)
        else present(); return end
    end
    if s.state=='dead' or s.state=='noGun' then present(); return end
    if s.reload_requested then s.reload_requested=false; reload(); present(); return end
    if s.ammo==0 and reserve()>0 and s.cooldown<=0 then reload(); present(); return end
    if is_run(s.state) then s.run_clock=s.run_clock+dt end
    -- Changing direction mid-sprint switches straight to that direction's state; its clip crossfades in.
    if is_run(s.state) and is_run(target) and s.state~=target then stable(target); present(); return end
    if (s.state=='lowReady' or s.state=='ads' or is_run(s.state)) and s.state~=target and not (s.state=='ads' and wants_shot() and not s.aim) then
        if carry(target,quick_fire) then present(); return end
    end
    if is_run(s.state) then present(); return end
    if s.state=='hipFire' and s.aim then pose_transition('hipToADS',1,.18,'ads'); present(); return end
    if s.state=='ads' and not s.aim and wants_shot() then pose_transition('adsToHip',HIP,.18,'hipFire'); present(); return end
    if s.state=='lowReady' and wants_shot() then
        local raise=quick_fire and setting('sprint_fire_raise_seconds',.14,.05,.5)
            or setting('hip_raise_seconds',.22,.05,1)
        pose_transition('hipRaise',HIP,raise,'hipFire'); present(); return
    end
    if s.cooldown<=0 and wants_shot() then
        shoot(cadence_remainder); s.pending=false; s.hip_shot=false; if s.burst>0 then s.burst=s.burst-1 end
    end
    if s.state=='hipFire' and s.recovery<=0 and not wants_shot() then pose_transition('hipLower',0,.65,'lowReady') end
    present()
end)
