-- Reusable gameplay hit detection. Zones are oriented boxes in metres relative
-- to the entity's feet; the collision worker still owns level geometry.
engine.hitbox = {}
local function finite(n) return type(n)=='number' and n==n and math.abs(n)<=10000000 end
local function vector(v)
    return type(v)=='table' and #v==3 and finite(v[1]) and finite(v[2]) and finite(v[3])
end
local function vec(x,y,z) local v=ecs.array(); v[1],v[2],v[3]=x,y,z; return v end
local function normal(x,y,z)
    local length=math.sqrt(x*x+y*y+z*z)
    if length<1e-12 then return vec(0,1,0) end
    return vec(x/length,y/length,z/length)
end
local function result(entity,a,b,t,n,zone)
    return {target=entity, fraction=t,
        position=vec(a[1]+(b[1]-a[1])*t,a[2]+(b[2]-a[2])*t,a[3]+(b[3]-a[3])*t),
        normal=n,zone=zone}
end

-- The legacy sphere form remains supported for existing scenes.
local function sphere(entity,h,pose,a,b)
    if not finite(h.radius) or h.radius<=0 or h.radius>1000 then return nil end
    local offset=vector(h.offset) and h.offset or {0,0,0}
    local x,y,z=pose.position[1]+offset[1],pose.position[2]+offset[2],pose.position[3]+offset[3]
    local dx,dy,dz=b[1]-a[1],b[2]-a[2],b[3]-a[3]
    local mx,my,mz=a[1]-x,a[2]-y,a[3]-z
    local aa=dx*dx+dy*dy+dz*dz
    local mb=mx*dx+my*dy+mz*dz
    local c=mx*mx+my*my+mz*mz-h.radius*h.radius
    local disc=mb*mb-aa*c
    local t=c<=0 and 0 or (disc>=0 and (-mb-math.sqrt(disc))/aa or nil)
    if not t or t<0 or t>1 then return nil end
    local nx,ny,nz=mx+dx*t,my+dy*t,mz+dz*t
    if nx*nx+ny*ny+nz*nz<1e-18 then nx,ny,nz=-dx,-dy,-dz end
    return result(entity,a,b,t,normal(nx,ny,nz))
end

-- Resolve one shared world-space frame for tracing and rendering. A Transform
-- is anchored at its feet; a camera derives feet from its collision body.
function engine.hitbox.frame(entity,h)
    if type(entity)~='string' or type(h)~='table' or h.enabled==false then return nil end
    local transform=ecs.get(entity,'Transform')
    local camera=ecs.get(entity,'PlayerCamera')
    local pose=transform or camera
    if not pose or not vector(pose.position) then return nil end
    local rig=ecs.get(entity,'PlayerRig')
    local body=ecs.get(entity,'PhysicsBody')
    local units=1
    if finite(h.units_per_meter) and h.units_per_meter>=.01 and h.units_per_meter<=100 then
        units=h.units_per_meter
    elseif h.scale_from_player_rig and rig and finite(rig.units_per_meter) and rig.units_per_meter>=.01 and rig.units_per_meter<=100 then
        units=rig.units_per_meter
    end
    local feet=pose.position[2]
    if h.anchor=='feet' and not transform and camera then
        if body and finite(body.radius) and finite(body.capsule_half_height) and finite(body.collision_offset_y) then
            feet=feet+body.collision_offset_y-body.radius-body.capsule_half_height
        elseif rig and finite(rig.eye_height_m) then feet=feet-rig.eye_height_m*units end
    end
    local height_scale=1
    if h.follow_crouch and body and rig and finite(rig.eye_height_m) and rig.eye_height_m>0 and
        finite(body.radius) and finite(body.capsule_half_height) and finite(body.collision_offset_y) then
        local eye=body.radius+body.capsule_half_height-body.collision_offset_y
        height_scale=math.max(.4,math.min(1.2,eye/(rig.eye_height_m*units)))
    end
    local yaw=finite(h.yaw) and h.yaw or (camera and finite(camera.yaw) and camera.yaw or 0)
    return {pose=pose,feet=feet,units=units,height_scale=height_scale,c=math.cos(yaw),s=math.sin(yaw)}
end

local function valid_zone(zone)
    return type(zone)=='table' and type(zone.name)=='string' and #zone.name>0 and #zone.name<=64 and
        vector(zone.center) and vector(zone.half_extents) and
        zone.half_extents[1]>0 and zone.half_extents[2]>0 and zone.half_extents[3]>0
end

-- Build the same oriented boxes used by hit detection as world-space edges.
-- Hosts can draw this bounded snapshot without owning any gameplay rules.
function engine.hitbox.visual_segments(entity,h,max_zones)
    local segments=ecs.array()
    local frame=engine.hitbox.frame(entity,h)
    if not frame or type(h.zones)~='table' then return segments end
    local count=finite(max_zones) and math.max(0,math.min(21,math.floor(max_zones))) or 21
    local edges={{1,2},{1,3},{1,5},{2,4},{2,6},{3,4},{3,7},{4,8},{5,6},{5,7},{6,8},{7,8}}
    for i=1,math.min(#h.zones,count) do
        local zone=h.zones[i]
        if valid_zone(zone) then
            local corners={}
            for x=-1,1,2 do for y=-1,1,2 do for z=-1,1,2 do
                local lx=(zone.center[1]+x*zone.half_extents[1])*frame.units
                local ly=(zone.center[2]+y*zone.half_extents[2])*frame.units*frame.height_scale
                local lz=(zone.center[3]+z*zone.half_extents[3])*frame.units
                corners[#corners+1]=vec(frame.pose.position[1]+frame.c*lx-frame.s*lz,
                    frame.feet+ly,frame.pose.position[3]+frame.s*lx+frame.c*lz)
            end end end
            for _,edge in ipairs(edges) do
                segments[#segments+1]={start=corners[edge[1]],finish=corners[edge[2]],zone=zone.name}
            end
        end
    end
    return segments
end

-- Returns the closest zone, including rays beginning inside a box. Boxes may
-- overlap; the first declared zone wins an exact tie.
function engine.hitbox.trace_segment(entity,h,a,b)
    if not vector(a) or not vector(b) then return nil end
    local dx,dy,dz=b[1]-a[1],b[2]-a[2],b[3]-a[3]
    if dx*dx+dy*dy+dz*dz<1e-20 then return nil end
    local frame=engine.hitbox.frame(entity,h)
    if not frame then return nil end
    if type(h.zones)~='table' then return sphere(entity,h,frame.pose,a,b) end
    local pose,feet,units,height_scale,c,s=frame.pose,frame.feet,frame.units,frame.height_scale,frame.c,frame.s
    local function local_point(v)
        local x,z=v[1]-pose.position[1],v[3]-pose.position[3]
        return {(c*x+s*z)/units,(v[2]-feet)/(units*height_scale),(-s*x+c*z)/units}
    end
    local start,finish=local_point(a),local_point(b)
    local d={finish[1]-start[1],finish[2]-start[2],finish[3]-start[3]}
    local best
    for i=1,math.min(#h.zones,64) do
        local zone=h.zones[i]
        if valid_zone(zone) then
            local entry,exit,axis,sign=0,1,nil,0
            for k=1,3 do
                local lo=zone.center[k]-zone.half_extents[k]
                local hi=zone.center[k]+zone.half_extents[k]
                if math.abs(d[k])<1e-12 then
                    if start[k]<lo or start[k]>hi then entry=2; break end
                else
                    local t1,t2=(lo-start[k])/d[k],(hi-start[k])/d[k]
                    local side=-1
                    if t1>t2 then t1,t2=t2,t1; side=1 end
                    if t1>entry then entry,axis,sign=t1,k,side end
                    exit=math.min(exit,t2)
                    if entry>exit then break end
                end
            end
            if entry<=exit and entry<=1 and exit>=0 and (not best or entry<best.fraction) then
                local nx,ny,nz=0,0,0
                if axis==1 then nx=sign elseif axis==2 then ny=sign elseif axis==3 then nz=sign
                else nx,ny,nz=-d[1],-d[2],-d[3] end
                local n=normal(c*nx-s*nz,ny/height_scale,s*nx+c*nz)
                best=result(entity,a,b,entry,n,zone.name)
            end
        end
    end
    return best
end
