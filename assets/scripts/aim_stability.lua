-- Reusable, deterministic aim stability model. Angles are in degrees; the
-- weapon controller projects them onto the same sight used by its shot ray.
engine.aim_stability={}

local function clamp(v,low,high) return math.max(low,math.min(high,v)) end
local function number(v,fallback,low,high)
    if type(v)~='number' or v~=v or math.abs(v)>10000000 then return fallback end
    return clamp(v,low,high)
end
local function setting(cfg,name,fallback,low,high)
    return number(cfg and cfg[name],fallback,low,high)
end

-- A model is per weapon; scripts may create independent instances for NPCs.
function engine.aim_stability.new(entity,receiver)
    local initial=ecs.get(entity,'AimStability')
    if not initial then return nil end
    local defaults={enabled=true,idle_sway_deg=.16,sway_frequency_hz=.38,walk_multiplier=2.2,
        crouch_multiplier=.6,ads_multiplier=.8,single_heat_per_shot=.04,burst_heat_per_shot=.10,
        auto_heat_per_shot=.18,heat_sway_deg=.45,heat_recovery_s=.9,under_fire_sway_deg=.55,
        under_fire_recovery_s=1.6,flinch_deg=.22,walk_recoil_multiplier=1.2,
        crouch_recoil_multiplier=.72,heat_recoil_multiplier=.35,response_s=.18}
    local entries={
        {'aim_stability','enabled','Aim stability'},
        {'aim_idle','idle_sway_deg','Idle sway (deg)',0,2},
        {'aim_frequency','sway_frequency_hz','Sway frequency (Hz)',.05,3},
        {'aim_walk','walk_multiplier','Walking sway multiplier',1,5},
        {'aim_crouch','crouch_multiplier','Crouch sway multiplier',.2,1.2},
        {'aim_ads','ads_multiplier','ADS sway multiplier',.2,1.5},
        {'aim_single_heat','single_heat_per_shot','Single shot instability',0,1},
        {'aim_burst_heat','burst_heat_per_shot','Burst shot instability',0,1},
        {'aim_auto_heat','auto_heat_per_shot','Auto shot instability',0,1},
        {'aim_heat_sway','heat_sway_deg','Sustained fire sway (deg)',0,3},
        {'aim_heat_recovery','heat_recovery_s','Fire recovery (s)',.1,5},
        {'aim_under_fire','under_fire_sway_deg','Under-fire sway (deg)',0,3},
        {'aim_pressure_recovery','under_fire_recovery_s','Under-fire recovery (s)',.1,8},
        {'aim_flinch','flinch_deg','Under-fire flinch (deg)',0,2},
        {'aim_walk_recoil','walk_recoil_multiplier','Walking recoil multiplier',1,3},
        {'aim_crouch_recoil','crouch_recoil_multiplier','Crouch recoil multiplier',.2,1},
        {'aim_heat_recoil','heat_recoil_multiplier','Sustained fire recoil',0,2},
        {'aim_response','response_s','Stability response (s)',.03,2},
    }
    -- Older scenes may provide only a subset of the component. Normalize once
    -- so all exported settings remain usable and agree with the model defaults.
    local changed=false
    for _,entry in ipairs(entries) do
        local field=entry[2]
        local value=initial[field]
        if entry[4] then value=number(value,defaults[field],entry[4],entry[5])
        elseif type(value)~='boolean' then value=defaults[field] end
        if initial[field]~=value then initial[field]=value; changed=true end
    end
    if changed then ecs.set(entity,'AimStability',initial) end
    local read=engine.component_reader(entity,'AimStability')
    for _,entry in ipairs(entries) do
        local spec={component='AimStability',field=entry[2],label=entry[3]}
        if entry[4] then spec.min,spec.max=entry[4],entry[5] end
        engine.export(entity,entry[1],spec)
    end

    local receiving=receiver and ecs.get(receiver,'SuppressionReceiver')
    if receiving then
        local radius=number(receiving.near_miss_radius_m,1.25,0,5)
        local enabled=receiving.enabled
        if type(enabled)~='boolean' then enabled=true end
        if receiving.near_miss_radius_m~=radius or receiving.enabled~=enabled then
            receiving.near_miss_radius_m,receiving.enabled=radius,enabled
            ecs.set(receiver,'SuppressionReceiver',receiving)
        end
        engine.export(receiver,'suppression_enabled',{component='SuppressionReceiver',
            field='enabled',label='Detect near misses'})
        engine.export(receiver,'suppression_radius',{component='SuppressionReceiver',
            field='near_miss_radius_m',label='Near-miss range (m)',min=0,max=5})
    end

    local model={heat=0,pressure=0,movement=0,crouch=0,aim=0,amplitude=0,
        kick_x=0,kick_y=0,clock=0,events=0,publish_elapsed=.1}
    local function cfg() return read() or initial end
    function model:reset()
        self.heat,self.pressure,self.movement,self.crouch,self.aim,self.amplitude=0,0,0,0,0,0
        self.kick_x,self.kick_y,self.clock,self.events,self.publish_elapsed=0,0,0,0,.1
        if ecs.get(entity,'AimStabilityState') then
            ecs.set(entity,'AimStabilityState',{movement=0,crouch=0,aim=0,heat=0,
                under_fire=0,sway_degrees=0,offset_x_degrees=0,offset_y_degrees=0})
        end
    end

    -- A near miss or direct hit can be supplied by any gameplay script.
    function model:under_fire(intensity,direct)
        local c=cfg()
        if c.enabled==false then return end
        intensity=number(intensity,1,0,1)
        if intensity<=0 then return end
        self.events=self.events+1
        self.pressure=clamp(self.pressure+intensity*(direct and .75 or .50),0,1)
        local walk=setting(c,'walk_recoil_multiplier',1.2,1,3)
        local crouch=setting(c,'crouch_recoil_multiplier',.72,.2,1)
        local stance=(1+(walk-1)*self.movement)*(1+(crouch-1)*self.crouch)
        local kick=setting(c,'flinch_deg',.22,0,2)*intensity*stance
        local side=self.events%2==0 and -1 or 1
        self.kick_x=clamp(self.kick_x+side*kick*.6,-2,2)
        self.kick_y=clamp(self.kick_y+kick*(direct and 1 or .6),-2,2)
    end

    -- Return a recoil multiplier and accumulate separate single/burst/auto heat.
    function model:shot(mode)
        local c=cfg()
        if c.enabled==false then return 1 end
        local key=mode=='auto' and 'auto_heat_per_shot' or mode=='burst' and 'burst_heat_per_shot' or 'single_heat_per_shot'
        local fallback=mode=='auto' and .18 or mode=='burst' and .10 or .04
        self.heat=clamp(self.heat+setting(c,key,fallback,0,1),0,1)
        local walk=setting(c,'walk_recoil_multiplier',1.2,1,3)
        local crouch=setting(c,'crouch_recoil_multiplier',.72,.2,1)
        local heat=setting(c,'heat_recoil_multiplier',.35,0,2)
        return (1+(walk-1)*self.movement)*(1+(crouch-1)*self.crouch)*(1+heat*self.heat)
    end

    -- Movement is collision-resolved speed relative to the player's walk speed.
    -- All envelopes recover exponentially, independent of the fixed tick rate.
    function model:update(dt,speed_ratio,crouch,aiming)
        local c=cfg()
        dt=number(dt,0,0,.25)
        if c.enabled==false then
            self.heat,self.pressure,self.movement,self.crouch,self.aim,self.amplitude=0,0,0,0,0,0
            self.kick_x,self.kick_y=0,0
            if self.was_enabled~=false then
                ecs.set(entity,'AimStabilityState',{movement=0,crouch=0,aim=0,heat=0,
                    under_fire=0,sway_degrees=0,offset_x_degrees=0,offset_y_degrees=0})
            end
            self.was_enabled=false
            return 0,0
        end
        if self.was_enabled==false then self.publish_elapsed=.1 end
        self.was_enabled=true
        local response=setting(c,'response_s',.18,.03,2)
        local ease=1-math.exp(-dt/response)
        self.movement=self.movement+(number(speed_ratio,0,0,1.5)-self.movement)*ease
        self.crouch=self.crouch+(number(crouch,0,0,1)-self.crouch)*ease
        self.aim=self.aim+((aiming and 1 or 0)-self.aim)*ease
        self.heat=self.heat*math.exp(-dt/setting(c,'heat_recovery_s',.9,.1,5))
        self.pressure=self.pressure*math.exp(-dt/setting(c,'under_fire_recovery_s',1.6,.1,8))
        self.kick_x=self.kick_x*math.exp(-dt/.18)
        self.kick_y=self.kick_y*math.exp(-dt/.18)
        local walk=setting(c,'walk_multiplier',2.2,1,5)
        local crouch_mul=setting(c,'crouch_multiplier',.6,.2,1.2)
        local ads=setting(c,'ads_multiplier',.8,.2,1.5)
        local stance=(1+(walk-1)*self.movement)*(1+(crouch_mul-1)*self.crouch)*(1+(ads-1)*self.aim)
        local desired=(setting(c,'idle_sway_deg',.16,0,2)+
            self.heat*setting(c,'heat_sway_deg',.45,0,3)+
            self.pressure*setting(c,'under_fire_sway_deg',.55,0,3))*stance
        self.amplitude=self.amplitude+(desired-self.amplitude)*ease
        self.clock=(self.clock+dt*setting(c,'sway_frequency_hz',.38,.05,3)*math.pi*2)%(math.pi*200)
        local x=self.amplitude*(.75*math.sin(self.clock)+.25*math.sin(self.clock*1.73+.7))+self.kick_x
        local y=self.amplitude*(.65*math.sin(self.clock*.81+1.1)+.20*math.sin(self.clock*1.41))+self.kick_y
        self.publish_elapsed=self.publish_elapsed+dt
        if self.publish_elapsed>=.1 then
            self.publish_elapsed=0
            ecs.set(entity,'AimStabilityState',{movement=self.movement,crouch=self.crouch,aim=self.aim,
                heat=self.heat,under_fire=self.pressure,sway_degrees=self.amplitude,
                offset_x_degrees=x,offset_y_degrees=y})
        end
        return x,y
    end
    return model
end
