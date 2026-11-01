// Browser Lua 5.4 host. Scripts use the same ECS operation names and scene event API as native Lua.
// The VM runs only on this browser thread; the ECS world remains owned by the Rust host.
// Match the native host's default instruction budget without charging time
// spent waiting inside JavaScript/Rust host calls against a healthy script.
const INSTRUCTION_LIMIT = 10_000_000;
const HOOK_INTERVAL = 1_000;
const BOOTSTRAP = `
local call, call_flat, call_changed, call_strings = __ecs_call, __ecs_call_flat, __ecs_call_changed, __ecs_call_strings
local type, pairs, ipairs = type, pairs, ipairs
local profile_now, profile_sample = __profile_now, __profile_sample
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
__ecs_call, __ecs_call_flat, __ecs_call_changed, __ecs_call_strings, __profile_now, __profile_sample = nil, nil, nil, nil, nil, nil
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
    if type(value) == "string" then return decode(call_strings(op, key, value)) end
    return invoke(op, {[key] = value})
end
local function invoke_two_strings(op, key1, value1, key2, value2)
    if type(value1) == "string" and type(value2) == "string" then
        return decode(call_strings(op, key1, value1, key2, value2))
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
function ecs.query(components) return invoke("query", { components = components }) end
function ecs.get(id, name) return invoke_two_strings("get", "id", id, "name", name) end
function ecs.get_if_changed(id, name, revision)
    if type(id) == "string" and type(name) == "string" and
        (revision == nil or type(revision) == "string") then
        local result = call_changed(id, name, revision)
        if result ~= false then return decode(result) end
        return nil
    end
    return invoke("get_if_changed", {id = id, name = name, revision = revision})
end
function ecs.get_many(id, names) return invoke("get_many", { id = id, names = names }) end
function ecs.has(id, name) return invoke_two_strings("has", "id", id, "name", name) end
function ecs.set(id, name, value) invoke("set", { id = id, name = name, value = value }) end
function ecs.set_field(id, name, field, value) invoke("set_field", { id = id, name = name, field = field, value = value }) end
function ecs.set_fields(id, name, fields) invoke("set_fields", { id = id, name = name, fields = fields }) end
function ecs.remove(id, name) return invoke_two_strings("remove", "id", id, "name", name) end
function ecs.get_resource(name) return invoke_one_string("get_resource", "name", name) end
function ecs.get_resource_field(name, field) return invoke_two_strings("get_resource_field", "name", name, "field", field) end
function ecs.set_resource(name, value) invoke("set_resource", { name = name, value = value }) end
function ecs.remove_resource(name) return invoke_one_string("remove_resource", "name", name) end
function ecs.set_parent(child, parent) invoke("set_parent", { child = child, parent = parent }) end
function ecs.parent(id) return invoke_one_string("parent", "id", id) end
function ecs.children(id) return invoke_one_string("children", "id", id) end
function ecs.emit(name, value) invoke("emit", { name = name, value = value }) end
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
        const request = {op:'get_if_changed', id, name};
        if (revision != null) request.revision = revision;
        const result = host.call(request, true);
        return result.found ? encodeResult(result.value) : false;
      });
      // Undefined becomes Lua nil, while JSON null remains ecs.null. Scalar
      // arguments avoid transferring a temporary Lua request table.
      vm.global.set('__ecs_call_strings', (op, key1, value1, key2, value2) => {
        const request = {op, [key1]:value1};
        if (key2 != null) request[key2] = value2;
        const result = host.call(request, true);
        return result.found ? encodeResult(result.value) : undefined;
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
    this.profile = profile ? {systems:Object.create(null), operations:Object.create(null)} : null;
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
  emit(name, value) { this.call({op: 'emit', name, value}); }
  drain(name) { return this.call({op: 'drain', name}).value; }
  // Drain and dispatch one channel at a time: a handler can emit an event for
  // a later channel and it will still be delivered in this pass.
  dispatchPending(names) {
    let start = 0;
    while (start < names.length) {
      const batch = this.call({op: 'drain_next', names, start}).value;
      if (batch === null) return;
      start = batch.index + 1;
      for (const event of batch.events) this.dispatch(names[batch.index], event);
    }
  }
  // Consume only the current channel before visiting the next. A consumer may
  // enqueue later channels, and an exception leaves those channels untouched.
  consumePending(names, consume) {
    let start = 0;
    while (start < names.length) {
      const batch = this.call({op: 'drain_next', names, start}).value;
      if (batch === null) return;
      start = batch.index + 1;
      consume(batch.index, batch.events);
    }
  }
  run(dt) { this.runSystems(dt); }
  dispatch(name, payload) { this.dispatchEvent(name, payload); }
  close() { if (this.vm) { this.vm.global.close(); this.vm = null; } }
}
