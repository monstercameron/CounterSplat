-- Weapon-owned optical data and reusable Lua API. The component describes the
-- equipped sight; the camera owns the smooth projection transition.
local ids = assert(ecs.get_resource("scene.entities"))
local default_weapon = ids.first_person
local valid = {iron=true, red_dot=true, scope=true}
engine.optics = engine.optics or {}
local function finite(value)
    return type(value) == "number" and value == value and math.abs(value) < math.huge
end
function engine.optics.magnification(optic)
    if type(optic) ~= "table" then return 1.25 end
    if optic.kind == "red_dot" then return 1.5 end
    if optic.kind == "scope" then
        local value = optic.scope_magnification
        return finite(value) and math.max(1, math.min(100, value)) or 1
    end
    return 1.25
end
-- Kind and scope power are stored per weapon, so later attachment systems can
-- equip a sight without knowing anything about the camera or UI.
function engine.optics.set(weapon, change)
    assert(type(weapon) == "string" and ecs.is_alive(weapon), "optic needs a live weapon")
    assert(type(change) == "table", "optic setting must be a table")
    local optic = ecs.get(weapon, "WeaponOptic") or {kind="iron", scope_magnification=4}
    local kind = change.kind
    if kind == nil then kind = optic.kind end
    local power = change.scope_magnification
    if power == nil then power = optic.scope_magnification end
    assert(valid[kind], "optic kind must be iron, red_dot, or scope")
    assert(finite(power) and power >= 1 and power <= 100, "scope magnification must be between 1 and 100")
    if optic.kind ~= kind or optic.scope_magnification ~= power or not ecs.has(weapon, "WeaponOptic") then
        optic.kind, optic.scope_magnification = kind, power
        ecs.set(weapon, "WeaponOptic", optic)
        ecs.emit("weapon.optic_changed", {entity=weapon, kind=kind, scope_magnification=power})
    end
    return optic
end
if default_weapon and ecs.has(default_weapon, "WeaponOptic") then
    engine.export(default_weapon, "scope_magnification", {component="WeaponOptic", field="scope_magnification",
        label="Scope magnification", min=1, max=100})
end
ecs.on("weapon.optic_set", function(event)
    if type(event) ~= "table" then return end
    local ok, err = pcall(engine.optics.set, event.entity or default_weapon, event)
    if not ok then ecs.emit("weapon.optic_error", {message=tostring(err)}) end
end)
