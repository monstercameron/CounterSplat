// Browser Lua 5.4 host. Scripts use the same ECS operation names and scene event API as native Lua.
// The VM runs only on this browser thread; the ECS world remains owned by the Rust host.
// Match the native host's default instruction budget without charging time
// spent waiting inside JavaScript/Rust host calls against a healthy script.
const INSTRUCTION_LIMIT = 10_000_000;
const HOOK_INTERVAL = 1_000;
// Invalid UTF-16 must keep the JSON path's escaping and validation behavior.
const isWellFormed = String.prototype.isWellFormed;
function directString(value, limit) {
  return value.length <= limit && typeof isWellFormed === 'function' && isWellFormed.call(value);
}
const BOOTSTRAP = `
local call, call_flat = __ecs_call, __ecs_call_flat
local call_changed, call_strings, query_one, get_pair = __ecs_call_changed, __ecs_call_strings, __ecs_query_one, __ecs_get_pair
local get_field = __ecs_get_field
local set_scalar, set_vector = __ecs_set_scalar, __ecs_set_vector
local set_pair, set_vector_pair, set_vector_triplet = __ecs_set_pair, __ecs_set_vector_pair, __ecs_set_vector_triplet
local emit_vector_triplet = __ecs_emit_vector_triplet
local type, pairs, ipairs, next_key, raw_get, get_meta = type, pairs, ipairs, next, rawget, getmetatable
local profile_now, profile_sample = __profile_now, __profile_sample
local huge = math.huge
local set_hook, raw_pcall, raw_xpcall, raise, select_value =
    debug.sethook, pcall, xpcall, error, select
local exhausted, remaining, budget_depth = {}, 0, 0
local function budget_hook()
    remaining = remaining - ${HOOK_INTERVAL}
    if remaining <= 0 then raise(exhausted, 0) end
end
local function preserve_results(ok, ...)
    if not ok and select_value(1, ...) == exhausted then raise(exhausted, 0) end
    return ok, ...
end
-- A script may catch ordinary errors, but cannot suppress the budget signal.
function pcall(fn, ...) return preserve_results(raw_pcall(fn, ...)) end
function xpcall(fn, handler, ...)
    if type(handler) ~= "function" then return raw_xpcall(fn, handler, ...) end
    return preserve_results(raw_xpcall(fn, function(err)
        if err == exhausted then return exhausted end
        return handler(err)
    end, ...))
end
local function bounded(fn, ...)
    local outer = budget_depth == 0
    if outer then
        remaining = ${INSTRUCTION_LIMIT}
        set_hook(budget_hook, "", ${HOOK_INTERVAL})
    end
    budget_depth = budget_depth + 1
    local ok, result = raw_pcall(fn, ...)
    budget_depth = budget_depth - 1
    if outer then set_hook() end
    if not ok then
        if result == exhausted then
            if outer then raise("Lua instruction budget exceeded", 0) end
            raise(exhausted, 0)
        end
        raise(result, 0)
    end
end
__ecs_call, __ecs_call_flat, __ecs_call_changed, __ecs_call_strings = nil, nil, nil, nil
__ecs_query_one, __ecs_get_pair, __ecs_set_scalar, __ecs_set_vector, __ecs_set_pair, __ecs_set_vector_pair, __ecs_set_vector_triplet = nil, nil, nil, nil, nil, nil, nil
__ecs_get_field = nil
__ecs_emit_vector_triplet = nil
__profile_now, __profile_sample = nil, nil
local arrays = setmetatable({}, { __mode = "k" })
local function encode(value, seen, depth, count)
    count.n = count.n + 1
    assert(depth <= 128 and count.n <= 100000, "ECS value is too large or deeply nested")
    if type(value) ~= "table" then return value end
    assert(not seen[value], "ECS values must be acyclic")
    seen[value] = true
    local result = {}
    if arrays[value] then result.__countersplat_array = true end
    for key, item in pairs(value) do result[key] = encode(item, seen, depth + 1, count) end
    seen[value] = nil
    return result
end
local function decode(value)
    if type(value) ~= "table" then return value end
    -- Each response owns its freshly transferred entry tables, so decode them
    -- in place instead of copying every object and array a second time.
    local result = value.entries
    if value.kind == "array" then
        arrays[result] = true
        for i, item in ipairs(result) do result[i] = decode(item) end
    elseif value.kind == "object" then
        for key, entry in pairs(result) do result[key] = decode(entry) end
    else
        error("invalid ECS bridge value")
    end
    return result
end
local function invoke(op, args)
    args = args or {}
    args.op = op
    local result = call(encode(args, {}, 0, {n = 0}))
    if result.found then return decode(result.value) end
end
-- Requests built here from strings have no nested tables to validate or mark.
-- Keep the general encoder for arbitrary Lua values and invalid argument types.
local function invoke_flat(op, args)
    args.op = op
    local result = call_flat(args)
    if result.found then return decode(result.value) end
end
local function invoke_one_string(op, key, value)
    if type(value) == "string" then
        local result = call_strings(op, key, value)
        if result ~= nil then return decode(result) end
        return
    end
    return invoke(op, {[key] = value})
end
local function invoke_two_strings(op, key1, value1, key2, value2)
    if type(value1) == "string" and type(value2) == "string" then
        local result = call_strings(op, key1, value1, key2, value2)
        if result ~= nil then return decode(result) end
        return
    end
    return invoke(op, {[key1] = value1, [key2] = value2})
end
ecs = { null = null }
function ecs.array() local result = {}; arrays[result] = true; return result end
function ecs.spawn() return invoke_flat("spawn", {}) end
function ecs.despawn(id, recursive) return invoke("despawn", { id = id, recursive = recursive or false }) end
function ecs.is_alive(id) return invoke_one_string("is_alive", "id", id) end
function ecs.entities() return invoke_flat("entities", {}) end
function ecs.hierarchy() return invoke_flat("hierarchy", {}) end
function ecs.structure_revision() return invoke_flat("structure_revision", {}) end
function ecs.components(id) return invoke_one_string("components", "id", id) end
function ecs.query(components)
    -- Ordinary one-name queries are common in systems. Keep metatables,
    -- extra keys, and all other shapes on the general validating path.
    if type(components) == "table" and get_meta(components) == nil then
        local component = raw_get(components, 1)
        if type(component) == "string" and next_key(components) == 1 and next_key(components, 1) == nil then
            local result = query_one(component)
            if result ~= nil then return decode(result) end
            return
        end
    end
    return invoke("query", { components = components })
end
function ecs.get(id, name) return invoke_two_strings("get", "id", id, "name", name) end
function ecs.get_field(id, name, field)
    if type(id) == "string" and type(name) == "string" and type(field) == "string" then
        local result = get_field(id, name, field)
        if result ~= nil then return decode(result) end
        return
    end
    return invoke("get_field", {id = id, name = name, field = field})
end
function ecs.get_if_changed(id, name, revision)
    if type(id) == "string" and type(name) == "string" and
        (revision == nil or type(revision) == "string") then
        local result = call_changed(id, name, revision)
        if result ~= false then return decode(result) end
        return
    end
    return invoke("get_if_changed", {id = id, name = name, revision = revision})
end
function ecs.get_many(id, names)
    if type(id) == "string" and type(names) == "table" and get_meta(names) == nil then
        local first, second = raw_get(names, 1), raw_get(names, 2)
        if type(first) == "string" and type(second) == "string" then
            local key1 = next_key(names)
            local key2 = next_key(names, key1)
            if ((key1 == 1 and key2 == 2) or (key1 == 2 and key2 == 1)) and next_key(names, key2) == nil then
                local result = get_pair(id, first, second)
                if result ~= nil then return decode(result) end
                return
            end
        end
    end
    return invoke("get_many", { id = id, names = names })
end
function ecs.has(id, name) return invoke_two_strings("has", "id", id, "name", name) end
function ecs.set(id, name, value) invoke("set", { id = id, name = name, value = value }) end
local function finite_number(value)
    return type(value) == "number" and value == value and value ~= huge and value ~= -huge
end
local function scalar_value(value)
    local kind = type(value)
    return kind == "string" or kind == "boolean" or finite_number(value)
end
-- Only ordinary tables with exactly three finite numbers can bypass encoding.
local function vector3(value)
    if type(value) ~= "table" or get_meta(value) ~= nil then return end
    local x, y, z = raw_get(value, 1), raw_get(value, 2), raw_get(value, 3)
    if not (finite_number(x) and finite_number(y) and finite_number(z)) then return end
    local key, count = next_key(value), 0
    while key ~= nil and (key == 1 or key == 2 or key == 3) do
        count = count + 1
        key = next_key(value, key)
    end
    if key == nil and count == 3 then return x, y, z end
end
-- Return scalar bridge arguments only for an ordinary object containing
-- exactly one finite vector and two scalars. The caller checks the keys.
local function vector_triplet(fields, first, second, third, a, b, a_scalar, b_scalar)
    local c = raw_get(fields, third)
    local vector, key, other1, value1, other2, value2, position
    if b_scalar and scalar_value(c) then
        vector, key, other1, value1, other2, value2, position = a, first, second, b, third, c, 1
    elseif a_scalar and scalar_value(c) then
        vector, key, other1, value1, other2, value2, position = b, second, first, a, third, c, 2
    elseif a_scalar and b_scalar then
        vector, key, other1, value1, other2, value2, position = c, third, first, a, second, b, 3
    end
    local x, y, z = vector3(vector)
    if x ~= nil then return key, x, y, z, other1, value1, other2, value2, position end
end
function ecs.set_field(id, name, field, value)
    if type(id) == "string" and type(name) == "string" and type(field) == "string" then
        if scalar_value(value) then
            set_scalar(id, name, field, value)
            return
        end
        local x, y, z = vector3(value)
        if x ~= nil then
            set_vector(id, name, field, x, y, z)
            return
        end
    end
    invoke("set_field", { id = id, name = name, field = field, value = value })
end
function ecs.set_fields(id, name, fields)
    -- Keep marked arrays, metatables, and unusual values on the validating path.
    if type(id) == "string" and type(name) == "string" and type(fields) == "table" and
        get_meta(fields) == nil and not arrays[fields] then
        local first = next_key(fields)
        if type(first) == "string" and first ~= "__countersplat_array" then
            local second = next_key(fields, first)
            if type(second) == "string" and second ~= "__countersplat_array" then
                local third = next_key(fields, second)
                local a, b = raw_get(fields, first), raw_get(fields, second)
                local a_scalar, b_scalar = scalar_value(a), scalar_value(b)
                if third == nil then
                    if a_scalar and b_scalar then
                        set_pair(id, name, first, a, second, b)
                        return
                    end
                    if b_scalar then
                        local x, y, z = vector3(a)
                        if x ~= nil then
                            set_vector_pair(id, name, first, x, y, z, second, b, true)
                            return
                        end
                    end
                    if a_scalar then
                        local x, y, z = vector3(b)
                        if x ~= nil then
                            set_vector_pair(id, name, second, x, y, z, first, a, false)
                            return
                        end
                    end
                elseif type(third) == "string" and third ~= "__countersplat_array" and next_key(fields, third) == nil then
                    -- An atomic vector plus two scalars is common for visual
                    -- transforms. Keep original key order for host consumers.
                    local key, x, y, z, other1, value1, other2, value2, position =
                        vector_triplet(fields, first, second, third, a, b, a_scalar, b_scalar)
                    if key ~= nil then
                        set_vector_triplet(id, name, key, x, y, z, other1, value1, other2, value2, position)
                        return
                    end
                end
            end
        end
    end
    invoke("set_fields", { id = id, name = name, fields = fields })
end
function ecs.remove(id, name) return invoke_two_strings("remove", "id", id, "name", name) end
function ecs.get_resource(name) return invoke_one_string("get_resource", "name", name) end
function ecs.get_resource_field(name, field) return invoke_two_strings("get_resource_field", "name", name, "field", field) end
function ecs.set_resource(name, value) invoke("set_resource", { name = name, value = value }) end
function ecs.remove_resource(name) return invoke_one_string("remove_resource", "name", name) end
function ecs.set_parent(child, parent) invoke("set_parent", { child = child, parent = parent }) end
function ecs.parent(id) return invoke_one_string("parent", "id", id) end
function ecs.children(id) return invoke_one_string("children", "id", id) end
function ecs.emit(name, value)
    -- Preserve the general path for arrays, metatables, extra keys, and all
    -- other values; this shortcut never exposes tables to the host callback.
    if type(name) == "string" and type(value) == "table" and
        get_meta(value) == nil and not arrays[value] then
        local first = next_key(value)
        if type(first) == "string" and first ~= "__countersplat_array" then
            local second = next_key(value, first)
            if type(second) == "string" and second ~= "__countersplat_array" then
                local third = next_key(value, second)
                if type(third) == "string" and third ~= "__countersplat_array" and next_key(value, third) == nil then
                    local a, b = raw_get(value, first), raw_get(value, second)
                    local key, x, y, z, other1, value1, other2, value2, position =
                        vector_triplet(value, first, second, third, a, b, scalar_value(a), scalar_value(b))
                    if key ~= nil then
                        emit_vector_triplet(name, key, x, y, z, other1, value1, other2, value2, position)
                        return
                    end
                end
            end
        end
    end
    invoke("emit", { name = name, value = value })
end
function ecs.drain(name) return invoke_one_string("drain", "name", name) end
function ecs.snapshot() return invoke_flat("snapshot", {}) end
local systems, order, handlers = {}, {}, {}
function ecs.system(name, callback)
    assert(type(name) == "string" and #name > 0 and type(callback) == "function", "invalid system")
    if not systems[name] then order[#order + 1] = name end
    systems[name] = callback
end
function ecs.remove_system(name)
    systems[name] = nil
    for i = #order, 1, -1 do if order[i] == name then table.remove(order, i) end end
end
function ecs.on(name, callback)
    assert(type(name) == "string" and #name > 0 and type(callback) == "function", "invalid handler")
    handlers[name] = handlers[name] or {}
    table.insert(handlers[name], callback)
end
function ecs.off(name) handlers[name] = nil end
local function run_systems(dt)
    local snapshot = { table.unpack(order) }
    for _, name in ipairs(snapshot) do
        if systems[name] then
            if profile_now and profile_sample then
                local started = profile_now()
                systems[name](dt)
                profile_sample(name, profile_now() - started)
            else
                systems[name](dt)
            end
        end
    end
end
local function dispatch(name, payload)
    local snapshot = { table.unpack(handlers[name] or {}) }
    for _, callback in ipairs(snapshot) do callback(payload) end
end
function __run_systems(dt) return bounded(run_systems, dt) end
function __dispatch(name, payload) return bounded(dispatch, name, payload) end
-- Match the native host's restricted libraries and filesystem policy.
io = nil; os = nil; package = nil; debug = nil; coroutine = nil
require = nil; dofile = nil; loadfile = nil; Promise = nil
`;

// Convert the JSON-shaped values crossing the bridge and reject cyclic or non-finite data.
function normalize(value, depth = 0, seen = new Set(), count = {value: 0}) {
  if (++count.value > 100000 || depth > 128) throw new Error('ECS value is too large or deeply nested');
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new Error('ECS values require finite numbers');
    return value;
  }
  if (typeof value !== 'object' || seen.has(value)) throw new Error('ECS values must be acyclic JSON');
  seen.add(value);
  let result;
  if (Array.isArray(value)) result = value.map(v => normalize(v, depth + 1, seen, count));
  else if (value.__countersplat_array === true) {
    const keys = Object.keys(value).filter(key => key !== '__countersplat_array');
    if (keys.some(key => !/^[1-9][0-9]*$/.test(key) || Number(key) > keys.length))
      throw new Error('ECS array must contain sequential entries');
    result = Array.from({length: keys.length}, (_, i) => normalize(value[i + 1], depth + 1, seen, count));
  } else {
    result = Object.create(null);
    for (const [key, entry] of Object.entries(value)) result[key] = normalize(entry, depth + 1, seen, count);
  }
  seen.delete(value);
  return result;
}

// Preserve JSON arrays (including empty ones) separately from objects across Wasmoon's table bridge.
function encodeResult(value) {
  if (Array.isArray(value)) return {kind: 'array', entries: value.map(encodeResult)};
  if (value !== null && typeof value === 'object') {
    // Objects keep their keys directly instead of allocating an entry table for
    // every field. A null prototype also preserves keys such as "__proto__".
    const entries = Object.create(null);
    for (const key of Object.keys(value)) entries[key] = encodeResult(value[key]);
    return {kind: 'object', entries};
  }
  return value;
}

// Immutable channel lists let a host reuse their serialized polling requests.
// Ordinary arrays remain supported when callers need to change the list.
export class EventChannels {
  #names;
  #requests = new Map();
  constructor(names) {
    if (!Array.isArray(names) || [...names].some(name => typeof name !== 'string'))
      throw new TypeError('Event channels must be an array of strings');
    this.#names = Object.freeze([...names]);
  }
  get names() { return this.#names; }
  request(start) {
    let encoded = this.#requests.get(start);
    if (encoded === undefined) {
      encoded = JSON.stringify({op:'drain_next', names:this.#names, start});
      // Large, dynamically chosen channel lists cannot grow this cache without bound.
      if (this.#requests.size < 64) this.#requests.set(start, encoded);
    }
    return encoded;
  }
}

export class LuaScene {
  static async create(factory, target, layer, engineApi, scripts, profile = false) {
    const vm = await factory.createEngine({openStandardLibs: true, enableProxy: false,
      injectObjects: true, traceAllocations: true});
    try {
      vm.global.setMemoryMax(32 * 1024 * 1024);
      const host = new LuaScene(vm, target, layer, profile);
      vm.global.set('__ecs_call', request => { const result = host.call(request); return {found: result.found, value: encodeResult(result.value)}; });
      // This callback is private to the bootstrap and accepts only internally
      // constructed requests whose arguments have been checked as strings.
      vm.global.set('__ecs_call_flat', request => { const result = host.call(request, true); return {found: result.found, value: encodeResult(result.value)}; });
      // Change-aware reads either return their result object or no change. A
      // boolean sentinel avoids transferring a wrapper table for the common
      // unchanged case; false cannot be a valid change record.
      vm.global.set('__ecs_call_changed', (id, name, revision) => {
        const result = host.readChanged(id, name, revision);
        return result === undefined ? false : encodeResult(result);
      });
      // Undefined becomes Lua nil, while JSON null remains ecs.null. Scalar
      // arguments avoid transferring a temporary Lua request table.
      vm.global.set('__ecs_call_strings', (op, key1, value1, key2, value2) => {
        if (op === 'get_resource' && key1 === 'name' && key2 == null &&
            typeof host.target.ecs_get_resource === 'function' && directString(value1, 256))
          return host.readValue(op, value1);
        if (op === 'get_resource_field' && key1 === 'name' && key2 === 'field' &&
            typeof host.target.ecs_get_resource_field === 'function' &&
            directString(value1, 256) && directString(value2, 256))
          return host.readValue(op, value1, value2);
        if (op === 'is_alive' && key1 === 'id' && key2 == null &&
            typeof host.target.ecs_is_alive === 'function' && directString(value1, 64))
          return host.readAlive(value1);
        if (op === 'has' && key1 === 'id' && key2 === 'name' &&
            typeof host.target.ecs_has_component === 'function' &&
            directString(value1, 64) && directString(value2, 256))
          return host.readHas(value1, value2);
        const request = {op, [key1]:value1};
        if (key2 != null) request[key2] = value2;
        const result = host.call(request, true);
        return result.found ? encodeResult(result.value) : undefined;
      });
      vm.global.set('__ecs_get_field', (id, name, field) => {
        if (typeof host.target.ecs_get_field === 'function' && directString(id, 64) &&
            directString(name, 256) && directString(field, 256)) return host.readField(id, name, field);
        if (host.profile) host.profile.bridge_paths.field_json++;
        const result = host.call({op:'get_field', id, name, field}, true);
        return result.found ? encodeResult(result.value) : undefined;
      });
      vm.global.set('__ecs_query_one', component => {
        const result = host.call({op:'query', components:[component]}, true);
        return result.found ? encodeResult(result.value) : undefined;
      });
      vm.global.set('__ecs_get_pair', (id, first, second) => {
        const result = host.call({op:'get_many', id, names:[first, second]}, true);
        return result.found ? encodeResult(result.value) : undefined;
      });
      // These callbacks receive only primitive values validated by the private
      // Lua wrappers. All other field values use the general JSON encoder.
      vm.global.set('__ecs_set_scalar', (id, name, field, value) => {
        host.call({op:'set_field', id, name, field, value}, true);
      });
      vm.global.set('__ecs_set_vector', (id, name, field, x, y, z) => {
        host.call({op:'set_field', id, name, field, value:[x, y, z]}, true);
      });
      vm.global.set('__ecs_set_pair', (id, name, first, a, second, b) => {
        const fields = Object.create(null);
        fields[first] = a; fields[second] = b;
        host.call({op:'set_fields', id, name, fields}, true);
      });
      vm.global.set('__ecs_set_vector_pair', (id, name, vector, x, y, z, other, value, vectorFirst) => {
        const fields = Object.create(null);
        if (vectorFirst) { fields[vector] = [x, y, z]; fields[other] = value; }
        else { fields[other] = value; fields[vector] = [x, y, z]; }
        host.call({op:'set_fields', id, name, fields}, true);
      });
      const vectorTriplet = (vector, x, y, z, first, a, second, b, position) => {
        const fields = Object.create(null);
        if (position === 1) fields[vector] = [x, y, z];
        fields[first] = a;
        if (position === 2) fields[vector] = [x, y, z];
        fields[second] = b;
        if (position === 3) fields[vector] = [x, y, z];
        return fields;
      };
      vm.global.set('__ecs_set_vector_triplet', (id, name, vector, x, y, z, first, a, second, b, position) => {
        host.call({op:'set_fields', id, name, fields:vectorTriplet(vector, x, y, z, first, a, second, b, position)}, true);
      });
      vm.global.set('__ecs_emit_vector_triplet', (name, vector, x, y, z, first, a, second, b, position) => {
        host.call({op:'emit', name, value:vectorTriplet(vector, x, y, z, first, a, second, b, position)}, true);
      });
      if (profile) {
        vm.global.set('__profile_now', () => performance.now());
        vm.global.set('__profile_sample', (name, elapsed) => host.recordProfile('systems', name, elapsed));
      }
      // Each independently authored script gets its own execution budget.
      // A single deadline for the entire scene makes an otherwise healthy
      // script fail just because earlier scripts or a busy GPU took time.
      const initialize = source => {
        vm.global.setTimeout(Date.now() + 500);
        try { vm.doStringSync(source); }
        finally { vm.global.setTimeout(undefined); }
      };
      initialize(BOOTSTRAP);
      initialize(engineApi);
      for (const source of scripts) initialize(source);
      host.runSystems = vm.global.get('__run_systems');
      host.dispatchEvent = vm.global.get('__dispatch');
      return host;
    } catch (error) { vm.global.close(); throw error; }
  }
  constructor(vm, target, layer, profile = false) {
    this.vm = vm; this.target = target; this.layer = layer;
    this.profile = profile ? {systems:Object.create(null), operations:Object.create(null),
      bridge_paths:{changed_direct:0, changed_json:0, alive_direct:0, has_direct:0,
        resource_direct:0, resource_field_direct:0, field_direct:0, field_json:0}} : null;
  }
  recordProfile(group, name, elapsed) {
    if (!this.profile || !Number.isFinite(elapsed) || elapsed < 0) return;
    const entry = this.profile[group][name] ||= {count:0, total_ms:0, max_ms:0};
    entry.count++; entry.total_ms += elapsed; entry.max_ms = Math.max(entry.max_ms, elapsed);
  }
  profileReport() { return this.profile; }
  call(request, scalarOnly = false) {
    const start = this.profile ? performance.now() : 0;
    const op = scalarOnly ? request : normalize(request);
    // An empty Lua table is ambiguous; a query explicitly expects a sequence.
    if (op.op === 'query' && !Array.isArray(op.components) && Object.keys(op.components || {}).length === 0) op.components = [];
    if (op.op === 'get_many' && !Array.isArray(op.names) && Object.keys(op.names || {}).length === 0) op.names = [];
    try { return JSON.parse(this.target.ecs_operation(this.layer, JSON.stringify(op))); }
    finally { if (this.profile) this.recordProfile('operations', op.op, performance.now()-start); }
  }
  readValue(op, first, second) {
    const start = this.profile ? performance.now() : 0;
    if (this.profile) this.profile.bridge_paths[op === 'get_resource' ? 'resource_direct' : 'resource_field_direct']++;
    let found = false, value;
    try {
      const raw = op === 'get_resource' ? this.target.ecs_get_resource(this.layer, first) :
        this.target.ecs_get_resource_field(this.layer, first, second);
      if (raw != null) { found = true; value = JSON.parse(raw); }
    } finally { if (this.profile) this.recordProfile('operations', op, performance.now()-start); }
    return found ? encodeResult(value) : undefined;
  }
  readField(id, name, field) {
    const start = this.profile ? performance.now() : 0;
    if (this.profile) this.profile.bridge_paths.field_direct++;
    try {
      const raw = this.target.ecs_get_field(this.layer, id, name, field);
      return raw == null ? undefined : encodeResult(JSON.parse(raw));
    } finally { if (this.profile) this.recordProfile('operations', 'get_field', performance.now()-start); }
  }
  readAlive(id) {
    const start = this.profile ? performance.now() : 0;
    if (this.profile) this.profile.bridge_paths.alive_direct++;
    try { return this.target.ecs_is_alive(this.layer, id); }
    finally { if (this.profile) this.recordProfile('operations', 'is_alive', performance.now()-start); }
  }
  readHas(id, name) {
    const start = this.profile ? performance.now() : 0;
    if (this.profile) this.profile.bridge_paths.has_direct++;
    try { return this.target.ecs_has_component(this.layer, id, name); }
    finally { if (this.profile) this.recordProfile('operations', 'has', performance.now()-start); }
  }
  readChanged(id, name, revision) {
    // Keep requests with unusual strings on the exact general protocol. The
    // small bounds also keep direct calls well under the JSON request budget.
    if (revision != null && typeof this.target.ecs_get_if_changed === 'function' &&
        directString(id, 64) && directString(name, 256) && directString(revision, 1024)) {
      const start = this.profile ? performance.now() : 0;
      if (this.profile) this.profile.bridge_paths.changed_direct++;
      try {
        const result = this.target.ecs_get_if_changed(this.layer, id, name, revision ?? undefined);
        return result == null ? undefined : JSON.parse(result);
      } finally { if (this.profile) this.recordProfile('operations', 'get_if_changed', performance.now()-start); }
    }
    if (this.profile) this.profile.bridge_paths.changed_json++;
    const request = {op:'get_if_changed', id, name};
    if (revision != null) request.revision = revision;
    const result = this.call(request, true);
    return result.found ? result.value : undefined;
  }
  // EventChannels requests have already been validated and encoded.
  callEncoded(op, encoded) {
    const start = this.profile ? performance.now() : 0;
    try { return JSON.parse(this.target.ecs_operation(this.layer, encoded)); }
    finally { if (this.profile) this.recordProfile('operations', op, performance.now()-start); }
  }
  emit(name, value) { this.call({op: 'emit', name, value}); }
  drain(name) { return this.call({op: 'drain', name}).value; }
  // Drain and dispatch one channel at a time: a handler can emit an event for
  // a later channel and it will still be delivered in this pass.
  dispatchPending(channels) {
    const fixed = channels instanceof EventChannels;
    const names = fixed ? channels.names : channels;
    let start = 0;
    while (start < names.length) {
      const batch = (fixed ? this.callEncoded('drain_next', channels.request(start))
                           : this.call({op:'drain_next', names, start})).value;
      if (batch === null) return;
      start = batch.index + 1;
      for (const event of batch.events) this.dispatch(names[batch.index], event);
    }
  }
  // Consume only the current channel before visiting the next. A consumer may
  // enqueue later channels, and an exception leaves those channels untouched.
  consumePending(channels, consume) {
    const fixed = channels instanceof EventChannels;
    const names = fixed ? channels.names : channels;
    let start = 0;
    while (start < names.length) {
      const batch = (fixed ? this.callEncoded('drain_next', channels.request(start))
                           : this.call({op:'drain_next', names, start})).value;
      if (batch === null) return;
      start = batch.index + 1;
      consume(batch.index, batch.events);
    }
  }
  run(dt) { this.runSystems(dt); }
  dispatch(name, payload) { this.dispatchEvent(name, payload); }
  close() { if (this.vm) { this.vm.global.close(); this.vm = null; } }
}
