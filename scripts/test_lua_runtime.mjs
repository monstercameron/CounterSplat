// Exercise the actual browser Lua 5.4 runtime and project scripts without a GPU.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {Worker} from 'node:worker_threads';
import {LuaScene, EventChannels} from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const read = path => readFile(new URL(path, import.meta.url), 'utf8');
const [engineApi, gameplay, menu] = await Promise.all([
  read('../crates/scripting-lua/src/engine_api.lua'),
  read('../assets/scripts/gameplay_navigation.lua'),
  read('../assets/scripts/menu_navigation.lua'),
]);

// A minimal transport fixture; Rust integration tests separately exercise the real ECS adapter.
class WorldFixture {
  constructor(ids) { this.resources = new Map([['scene.entities', ids]]); this.events = new Map(); this.components = new Map(); this.revision = 0; }
  ecs_operation(_layer, encoded) {
    const op = JSON.parse(encoded);
    let value, found = true;
    switch (op.op) {
      case 'hierarchy': value = [{id:'root', parent:null}, {id:'child', parent:'root'}]; break;
      case 'structure_revision': value = '0'; break;
      case 'get': {
        const components = this.components.get(op.id) || new Map();
        found = components.has(op.name);
        value = components.get(op.name);
        break;
      }
      case 'set_fields': {
        const component = (this.components.get(op.id) || new Map()).get(op.name);
        if (component === null || typeof component !== 'object' || Array.isArray(component) ||
            op.fields === null || typeof op.fields !== 'object' || Array.isArray(op.fields))
          throw new Error('component and fields must be objects');
        Object.assign(component, op.fields);
        this.revision++;
        found = false;
        break;
      }
      case 'set_field': {
        const component = (this.components.get(op.id) || new Map()).get(op.name);
        if (component === null || typeof component !== 'object' || Array.isArray(component))
          throw new Error('component is missing or not an object');
        component[op.field] = op.value;
        this.revision++;
        found = false;
        break;
      }
      case 'has': value = (this.components.get(op.id) || new Map()).has(op.name); break;
      case 'is_alive': value = this.components.has(op.id); break;
      case 'get_if_changed': {
        if (op.id === 'stale') throw new Error('stale entity');
        const components = this.components.get(op.id) || new Map();
        if (op.name === 'Untracked') {
          value = {revision:null, present:true, value:{number:this.revision}};
          break;
        }
        const revision = `${op.id}|${op.name}|${this.revision}`;
        if (op.revision === revision) { found = false; break; }
        value = {revision, present:components.has(op.name), value:components.get(op.name) ?? null};
        break;
      }
      case 'get_many': {
        if (op.id === 'stale') throw new Error('stale entity');
        if (op.id === 'no-result') { found = false; break; }
        value = Object.create(null);
        const components = this.components.get(op.id) || new Map();
        for (const name of op.names) if (components.has(name)) value[name] = components.get(name);
        break;
      }
      case 'get_resource': found = this.resources.has(op.name); value = this.resources.get(op.name); break;
      case 'get_resource_field': {
        const resource = this.resources.get(op.name);
        found = resource !== null && typeof resource === 'object' && Object.hasOwn(resource, op.field);
        value = found ? resource[op.field] : null; break;
      }
      case 'set_resource': this.resources.set(op.name, op.value); found = false; break;
      case 'emit': if (op.name === 'reject') throw new Error('rejected event'); this.events.set(op.name, [...(this.events.get(op.name) || []), op.value]); found = false; break;
      case 'drain': value = this.events.get(op.name) || []; this.events.delete(op.name); break;
      case 'drain_next': {
        found = false;
        for (let index = op.start || 0; index < op.names.length; index++) {
          const events = this.events.get(op.names[index]);
          if (events?.length) {
            value = {index, events}; this.events.delete(op.names[index]); found = true; break;
          }
        }
        break;
      }
      default: throw new Error(`Unexpected ECS operation ${op.op}`);
    }
    return JSON.stringify({found, value: value ?? null});
  }
}
const factory = new LuaFactory();
const world = new WorldFixture({back_to_levels: 'game-levels'});
const game = await LuaScene.create(factory, world, 'gameplay', engineApi, [gameplay]);
assert.equal(game.vm.doStringSync('return _VERSION'), 'Lua 5.4');
function runGameplay() {
  for (const name of ['input.key', 'ui.action']) for (const event of game.drain(name)) game.dispatch(name, event);
  game.run(1 / 60);
}
game.emit('input.key', {key: 'Escape', pressed: true, is_repeat: true});
runGameplay();
assert.deepEqual(game.drain('engine.scene'), []);
game.emit('input.key', {key: 'Escape', pressed: true, is_repeat: false});
runGameplay();
assert.deepEqual(game.drain('engine.scene'), [{action:'push', scene:'pause_menu'}]);
game.emit('ui.action', {id:'game-levels', kind:{type:'clicked'}});
runGameplay();
assert.deepEqual(game.drain('engine.scene'), [{action:'replace', scene:'level_selector'}]);
world.components.set('entity', new Map([['Health', 50], ['Nullable', null], ['False', false], ['__proto__', 'safe'], ['Stats', Object.assign(Object.create(null), {hp:10, keep:true})]]));
world.resources.set('null-resource', null);
world.resources.set('false-resource', false);
world.resources.set('special-keys', JSON.parse('{"__proto__":{"items":[{},[],null]},"constructor":"safe","kind":"user","entries":"data"}'));
world.resources.set('nested-roundtrip', JSON.parse('{"rows":[{"name":"original","enabled":false},[],null],"empty":{},"empty_list":[],"zero":0}'));
game.vm.doStringSync(`
  local count = select('#', pcall(function() return 1, nil, 3 end))
  assert(count == 4)
  local ok, value = xpcall(function() error('ordinary') end, function() return 'handled' end)
  assert(not ok and value == 'handled')
  assert(not pcall(function() xpcall(function() end, nil) end))
  assert(__ecs_call == nil and __ecs_call_flat == nil and __ecs_call_changed == nil and __ecs_call_strings == nil and __ecs_query_one == nil and __ecs_get_pair == nil and __ecs_set_scalar == nil and __ecs_set_vector == nil and __ecs_set_pair == nil and __ecs_set_vector_pair == nil and __ecs_set_vector_triplet == nil and __ecs_emit_vector_triplet == nil)
  assert(ecs.is_alive('entity') and not ecs.is_alive('missing'))
  assert(ecs.get('entity', 'Health') == 50 and ecs.has('entity', 'Health'))
  assert(ecs.get('entity', 'Nullable') == ecs.null and ecs.get('entity', 'Missing') == nil)
  assert(ecs.get('entity', 'False') == false)
  assert(select('#', ecs.get('entity', 'Missing')) == 0)
  assert(select('#', ecs.get('entity', 'Nullable')) == 1)
  assert(select('#', ecs.get('entity', 'False')) == 1)
  assert(ecs.get_resource('null-resource') == ecs.null and ecs.get_resource('false-resource') == false)
  assert(ecs.get_resource('missing-resource') == nil)
  assert(select('#', ecs.get_resource('missing-resource')) == 0)
  assert(select('#', ecs.get_resource('null-resource')) == 1)
  assert(ecs.get('entity', '__proto__') == 'safe' and not ecs.has('entity', 'Missing'))
  ecs.set_field('entity', 'Stats', 'hp', 25)
  ecs.set_field('entity', 'Stats', 'optional', ecs.null)
  ecs.set_field('entity', 'Stats', 'position', {1, -2, 3.5})
  local vector = ecs.array(); vector[1], vector[2], vector[3] = 4, 5, 6
  ecs.set_field('entity', 'Stats', 'velocity', vector)
  ecs.set_field('entity', 'Stats', 'flag', false)
  ecs.set_field('entity', 'Stats', 'label', 'safe')
  local wrapped = setmetatable({1,2,3}, {__pairs=function() return next, {7,8,9}, nil end})
  ecs.set_field('entity', 'Stats', 'wrapped', wrapped)
  ecs.set_field('entity', 'Stats', 'object', {[1]=1, [2]=2, [3]=3, extra=4})
  local sparse = ecs.array(); sparse[1], sparse[3] = 1, 3
  assert(not pcall(function() ecs.set_field('entity', 'Stats', 'sparse', sparse) end))
  ecs.set_field('entity', 'Stats', '__proto__', 'safe')
  ecs.set_fields('entity', 'Stats', {hp=30, optional=ecs.null})
  ecs.set_fields('entity', 'Stats', {position={7,8,9}, flag=true})
  ecs.set_fields('entity', 'Stats', {label='paired', ['__proto__']='paired-safe'})
  -- Exercise the vector in each traversal position, including a reserved JS key.
  local triple = {left=1, right=false, ['__proto__']='safe'}
  local keys, key = {}, next(triple)
  while key do keys[#keys+1], key = key, next(triple, key) end
  for i=1,3 do
    triple[keys[1]], triple[keys[2]], triple[keys[3]] = 1, false, 'safe'
    triple[keys[i]] = {i, i+1, i+2}
    ecs.set_fields('entity', 'Stats', triple)
    local actual = ecs.get('entity', 'Stats')
    assert(actual[keys[i]][1] == i and actual[keys[i]][3] == i+2)
    for j=1,3 do if j ~= i then assert(actual[keys[j]] == triple[keys[j]]) end end
  end
  local marked = ecs.array(); marked[1], marked[2], marked[3] = 4, 5, 6
  ecs.set_fields('entity', 'Stats', {triple_vector=marked, triple_a=true, triple_b='ok'})
  local triple_proxy = setmetatable({a={1,2,3}, b=0, c=0}, {
    __pairs = function() return next, {a={9,8,7}, b=2, c=3}, nil end
  })
  ecs.set_fields('entity', 'Stats', triple_proxy)
  local triple_stats = ecs.get('entity', 'Stats')
  assert(triple_stats.triple_vector[2] == 5 and triple_stats.triple_a and triple_stats.triple_b == 'ok')
  assert(triple_stats.a[1] == 9 and triple_stats.b == 2 and triple_stats.c == 3)
  local wrapped_vector = setmetatable({1,2,3}, {
    __pairs = function() return next, {8,7,6}, nil end
  })
  ecs.set_fields('entity', 'Stats', {wrapped_vector=wrapped_vector, one=1, two=2})
  ecs.set_fields('entity', 'Stats', {extra_vector={[1]=1,[2]=2,[3]=3,extra=4}, one=1, two=2})
  ecs.set_fields('entity', 'Stats', {four_vector={1,2,3}, one=1, two=2, three=3})
  ecs.set_fields('entity', 'Stats', {null_vector={1,2,3}, one=ecs.null, two=false})
  triple_stats = ecs.get('entity', 'Stats')
  assert(triple_stats.wrapped_vector[1] == 8 and triple_stats.extra_vector.extra == 4)
  assert(triple_stats.four_vector[3] == 3 and triple_stats.three == 3)
  assert(triple_stats.null_vector[2] == 2 and triple_stats.one == ecs.null and not triple_stats.two)
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', {__countersplat_array=true, one=1, two=2}) end))
  local marked_fields = ecs.array(); marked_fields[1], marked_fields[2], marked_fields[3] = 1, 2, 3
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', marked_fields) end))
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', {x={1,2,3}, y=0/0, z=1}) end))
  assert(not pcall(function() ecs.set_fields('entity', 'Missing', {x={1,2,3}, y=0, z=1}) end))
  ecs.set_field('entity', 'Stats', '__proto__', 'paired-safe')
  -- Each traversal position uses the same event fast path, with stable
  -- snapshots and special keys. Unsupported values retain normal encoding.
  local event = {left='entity', right=false, ['__proto__']=true}
  keys, key = {}, next(event)
  while key do keys[#keys+1], key = key, next(event, key) end
  for i=1,3 do
    event[keys[1]], event[keys[2]], event[keys[3]] = 'entity', false, true
    event[keys[i]] = {i, i+1, i+2}
    ecs.emit('triplet', event)
    local result = ecs.drain('triplet')[1]
    assert(result[keys[i]][1] == i and result[keys[i]][3] == i+2)
    for j=1,3 do if j ~= i then assert(result[keys[j]] == event[keys[j]]) end end
  end
  local event_vector = ecs.array(); event_vector[1], event_vector[2], event_vector[3] = 4,5,6
  ecs.emit('triplet', {vector=event_vector, text='safe', flag=false})
  event_vector[1] = 99
  local event_result = ecs.drain('triplet')[1]
  assert(event_result.vector[1] == 4 and event_result.text == 'safe' and not event_result.flag)
  ecs.emit('fallback', {vector=wrapped_vector, one=1, two=2})
  ecs.emit('fallback', {vector={1,2,3, extra=4}, one=ecs.null, two=false})
  ecs.emit('fallback', {one=1, two=2, three=3, four=4})
  ecs.emit('fallback', triple_proxy)
  local fallback = ecs.drain('fallback')
  assert(fallback[1].vector[1] == 8 and fallback[2].vector.extra == 4)
  assert(fallback[2].one == ecs.null and fallback[3].four == 4)
  assert(fallback[4].a[1] == 9 and fallback[4].b == 2)
  assert(select('#', ecs.emit('triplet', {v={1,2,3}, a=1, b=false})) == 0)
  assert(not pcall(function() ecs.emit('reject', {v={1,2,3}, a=1, b=false}) end))
  assert(not pcall(function() ecs.emit('bad', {v={1,2,3}, a=0/0, b=1}) end))
  assert(not pcall(function() ecs.emit('bad', {__countersplat_array=true, a=1, b=2}) end))
  local fields_proxy = setmetatable({hp=999, keep=false}, {
    __pairs = function() return next, {hp=30, keep=true}, nil end
  })
  ecs.set_fields('entity', 'Stats', fields_proxy)
  assert(not pcall(function()
    ecs.set_fields('entity', 'Stats', {__countersplat_array=true, hp=5})
  end))
  ecs.set_fields('entity', 'Stats', {})
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', ecs.array()) end))
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', {hp=0/0}) end))
  local patch_cycle = {}; patch_cycle.self = patch_cycle
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', patch_cycle) end))
  local stats = ecs.get('entity', 'Stats')
  assert(stats.hp == 30 and stats.keep and stats.optional == ecs.null and stats.__proto__ == 'paired-safe')
  assert(stats.position[2] == 8 and stats.position[3] == 9 and stats.velocity[1] == 4)
  assert(stats.flag == true and stats.label == 'paired' and stats.wrapped[1] == 7)
  assert(stats.object.extra == 4)
  local changed = ecs.get_if_changed('entity', 'Stats')
  assert(changed.present and changed.value.hp == 30 and type(changed.revision) == 'string')
  assert(ecs.get_if_changed('entity', 'Stats', changed.revision) == nil)
  assert(select('#', ecs.get_if_changed('entity', 'Stats', changed.revision)) == 0)
  local bad_revision = {}; bad_revision.self = bad_revision
  assert(not pcall(function() ecs.get_if_changed('entity', 'Stats', bad_revision) end))
  ecs.set_field('entity', 'Stats', 'hp', 31)
  local newer = ecs.get_if_changed('entity', 'Stats', changed.revision)
  assert(newer.present and newer.value.hp == 31 and newer.revision ~= changed.revision)
  local missing = ecs.get_if_changed('entity', 'Missing')
  assert(not missing.present and missing.value == ecs.null)
  local read_stats = engine.component_reader('entity', 'Stats')
  local cached = read_stats()
  assert(cached.hp == 31 and read_stats() == cached)
  local editable = ecs.get('entity', 'Stats'); editable.hp = 100
  assert(read_stats().hp == 31 and ecs.get('entity', 'Stats').hp == 31)
  ecs.set_field('entity', 'Stats', 'hp', 32)
  assert(read_stats().hp == 32 and read_stats() ~= cached)
  assert(engine.component_reader('entity', 'Nullable')() == ecs.null)
  assert(engine.component_reader('entity', 'Missing')() == nil)
  assert(engine.component_reader('entity', 'False')() == false)
  local read_untracked = engine.component_reader('entity', 'Untracked')
  local untracked1, untracked2 = read_untracked(), read_untracked()
  assert(untracked1.number == untracked2.number and untracked1 ~= untracked2)
  assert(not pcall(function() ecs.get_if_changed('stale', 'Stats') end))
  assert(not pcall(function() ecs.set_field('entity', 'Missing', 'hp', 1) end))
  assert(not pcall(function() ecs.set_fields('entity', 'Missing', {position={1,2,3}, flag=true}) end))
  assert(not pcall(function() ecs.set_field('entity', 'Stats', 'bad', 0/0) end))
  assert(not pcall(function() ecs.set_field('entity', 'Stats', 'bad', math.huge) end))
  -- Non-string arguments still use the validating general encoder.
  local cycle = {}; cycle.self = cycle
  assert(not pcall(function() ecs.get(cycle, 'Health') end))
  assert(not pcall(function() ecs.get_resource_field('scene.entities', cycle) end))
  assert(not pcall(function() ecs.get_resource(0/0) end))
  -- Replacing Lua globals must not let a nested value into the scalar-only callback.
  local saved_type, saved_pairs = type, pairs
  type = function() return 'string' end
  local accepted_cycle = pcall(function() ecs.get(cycle, 'Health') end)
  type = saved_type
  assert(not accepted_cycle)
  pairs = function() return function() return nil end end
  assert(ecs.get('entity', 'Health') == 50)
  pairs = saved_pairs
  local pair = ecs.get_many('entity', {'Health', 'Nullable'})
  assert(pair.Health == 50 and pair.Nullable == ecs.null)
  local pair_array = ecs.array(); pair_array[1], pair_array[2] = 'Health', 'Missing'
  assert(ecs.get_many('entity', pair_array).Health == 50)
  assert(ecs.get_many('entity', {'Health', 'Health'}).Health == 50)
  assert(ecs.get_many('entity', {'__proto__', 'Nullable'}).__proto__ == 'safe')
  assert(select('#', ecs.get_many('no-result', {'Health', 'Nullable'})) == 0)
  assert(not pcall(function() ecs.get_many('stale', {'Health', 'Nullable'}) end))
  local malformed = ecs.array(); malformed[1], malformed[2], malformed.extra = 'Health', 'Nullable', true
  assert(not pcall(function() ecs.get_many('entity', malformed) end))
  local pair_proxy = setmetatable({'wrong', 'wrong'}, {
    __pairs = function() return next, {'Health', 'Nullable'}, nil end
  })
  assert(ecs.get_many('entity', pair_proxy).Nullable == ecs.null)
  local pair_cycle = {}; pair_cycle.self = pair_cycle
  assert(not pcall(function() ecs.get_many('entity', pair_cycle) end))
  local batch = ecs.get_many('entity', {'Health', 'Nullable', 'Missing', '__proto__'})
  assert(batch.Health == 50 and batch.Nullable == ecs.null and batch.Missing == nil)
  assert(batch.__proto__ == 'safe' and next(ecs.get_many('entity', {})) == nil)
  local special = ecs.get_resource('special-keys')
  assert(special.__proto__.items[1] ~= nil and #special.__proto__.items[2] == 0)
  assert(special.__proto__.items[3] == ecs.null)
  assert(special.constructor == 'safe' and special.kind == 'user' and special.entries == 'data')
  local first = ecs.get_resource('nested-roundtrip')
  local second = ecs.get_resource('nested-roundtrip')
  first.rows[1].name = 'changed'
  first.rows[2][1] = 'added'
  first.empty.key = true
  first.empty_list[1] = false
  assert(second.rows[1].name == 'original' and #second.rows[2] == 0)
  assert(second.empty.key == nil and #second.empty_list == 0 and second.rows[3] == ecs.null)
  ecs.set_resource('nested-changed', first)
`);
game.vm.doStringSync(`
  ecs.set_resource('roundtrip', {items = ecs.array(), marker = ecs.null, nested = {1, 2}})
  local tree = ecs.hierarchy()
  assert(ecs.structure_revision() == '0')
  assert(#tree == 2 and tree[1].parent == ecs.null and tree[2].parent == 'root')
  local value = ecs.get_resource('roundtrip')
  assert(#value.items == 0 and value.marker == ecs.null and value.nested[2] == 2)
  assert(ecs.get_resource_field('roundtrip','marker') == ecs.null)
  assert(ecs.get_resource_field('roundtrip','missing') == nil)
  ecs.set_resource('again', value)
  assert(not pcall(function() ecs.set_resource('bad', 0/0) end))
  local cycle = {}; cycle.self = cycle
  assert(not pcall(function() ecs.set_resource('bad', cycle) end))
`);
assert.deepEqual(world.resources.get('again'), {items:[], marker:null, nested:[1, 2]});
assert.deepEqual(world.resources.get('nested-changed'), {
  rows:[{name:'changed',enabled:false},['added'],null], empty:{key:true}, empty_list:[false], zero:0,
});
assert.deepEqual(world.resources.get('nested-roundtrip'), {
  rows:[{name:'original',enabled:false},[],null], empty:{}, empty_list:[], zero:0,
});

game.close();

// Exercise every scalar-only wrapper and compare the requests received by the host.
const calls = [];
const responses = {
  spawn: 'new-entity', is_alive: true, entities: ['entity'],
  hierarchy: [{id:'entity', parent:null}], structure_revision: '7',
  components: ['Name'], get: {value:'ok'}, has: true, remove: 'removed',
  get_if_changed: {revision:'fresh', present:true, value:{value:'changed'}},
  get_resource: {value:'resource'}, get_resource_field: 'field-value',
  remove_resource: 'removed-resource', parent: 'parent', children: ['child'],
  drain: [{value:1}], snapshot: {entities:[]},
};
const scalarWorld = {ecs_operation(_layer, encoded) {
  const op = JSON.parse(encoded);
  calls.push(op);
  return JSON.stringify({found:true, value:responses[op.op]});
}};
const scalar = await LuaScene.create(factory, scalarWorld, 'gameplay', engineApi, []);
scalar.vm.doStringSync(`
  assert(ecs.spawn() == 'new-entity' and ecs.is_alive('entity'))
  assert(ecs.entities()[1] == 'entity' and ecs.hierarchy()[1].parent == ecs.null)
  assert(ecs.structure_revision() == '7' and ecs.components('entity')[1] == 'Name')
  assert(ecs.get('entity', 'Name').value == 'ok' and ecs.has('entity', 'Name'))
  assert(ecs.get_if_changed('entity', 'Name').value.value == 'changed')
  assert(ecs.get_if_changed('entity', 'Name', 'old').revision == 'fresh')
  assert(ecs.remove('entity', 'Name') == 'removed')
  assert(ecs.get_resource('settings').value == 'resource')
  assert(ecs.get_resource_field('settings', 'field') == 'field-value')
  assert(ecs.remove_resource('settings') == 'removed-resource')
  assert(ecs.parent('entity') == 'parent' and ecs.children('entity')[1] == 'child')
  assert(ecs.drain('event')[1].value == 1 and #ecs.snapshot().entities == 0)
`);
assert.deepEqual(calls, [
  {op:'spawn'}, {op:'is_alive', id:'entity'}, {op:'entities'}, {op:'hierarchy'},
  {op:'structure_revision'}, {op:'components', id:'entity'},
  {op:'get', id:'entity', name:'Name'}, {op:'has', id:'entity', name:'Name'},
  {op:'get_if_changed', id:'entity', name:'Name'},
  {op:'get_if_changed', id:'entity', name:'Name', revision:'old'},
  {op:'remove', id:'entity', name:'Name'}, {op:'get_resource', name:'settings'},
  {op:'get_resource_field', name:'settings', field:'field'},
  {op:'remove_resource', name:'settings'}, {op:'parent', id:'entity'},
  {op:'children', id:'entity'}, {op:'drain', name:'event'}, {op:'snapshot'},
]);
scalar.close();

// One-name query optimization preserves ordinary, explicitly marked, and
// metatable-driven queries, while invalid shapes still reach validation.
const queryCalls = [];
const queryWorld = {ecs_operation(_layer, encoded) {
  const op = JSON.parse(encoded);
  queryCalls.push(op);
  if (op.op !== 'query' || !Array.isArray(op.components)) throw new Error('invalid query');
  if (op.components[0] === 'absent') return JSON.stringify({found:false, value:null});
  return JSON.stringify({found:true, value:['entity']});
}};
const queryScene = await LuaScene.create(factory, queryWorld, 'gameplay', engineApi, []);
queryScene.vm.doStringSync(`
  assert(ecs.query({'PhysicsBody'})[1] == 'entity')
  local marked = ecs.array(); marked[1] = 'PlayerCamera'
  assert(ecs.query(marked)[1] == 'entity')
  local proxy = setmetatable({'ignored'}, {
    __pairs = function() return next, {'Transform'}, nil end
  })
  assert(ecs.query(proxy)[1] == 'entity')
  assert(ecs.query({'PhysicsBody', 'Transform'})[1] == 'entity')
  assert(ecs.query(ecs.array())[1] == 'entity')
  assert(select('#', ecs.query({'absent'})) == 0)
  assert(not pcall(function() ecs.query({[1]='PhysicsBody', extra='invalid'}) end))
  local cycle = {}; cycle.self = cycle
  assert(not pcall(function() ecs.query(cycle) end))
  local saved_next, saved_rawget, saved_getmetatable = next, rawget, getmetatable
  next = function() return 1 end
  rawget = function() return 'PhysicsBody' end
  getmetatable = function() return nil end
  assert(not pcall(function() ecs.query(cycle) end))
  next, rawget, getmetatable = saved_next, saved_rawget, saved_getmetatable
`);
assert.deepEqual(queryCalls.slice(0, 5), [
  {op:'query', components:['PhysicsBody']},
  {op:'query', components:['PlayerCamera']},
  {op:'query', components:['Transform']},
  {op:'query', components:['PhysicsBody','Transform']},
  {op:'query', components:[]},
]);
queryScene.close();

// Dispatching a channel may enqueue events for earlier, current, or later
// channels. Only later channels are delivered during this pass.
const orderedWorld = new WorldFixture({});
const ordered = await LuaScene.create(factory, orderedWorld, 'gameplay', engineApi, [`
  ecs.on('earlier', function() ecs.emit('seen', {name='earlier'}) end)
  ecs.on('first', function(event)
    if event.initial then
      ecs.emit('earlier', {})
      ecs.emit('first', {initial=false})
      ecs.emit('later', {name='later'})
    else
      ecs.emit('seen', {name='first'})
    end
  end)
  ecs.on('later', function(event) ecs.emit('seen', event) end)
`]);
ordered.emit('first', {initial:true});
ordered.dispatchPending(['earlier', 'first', 'later']);
assert.deepEqual(ordered.drain('seen'), [{name:'later'}]);
ordered.dispatchPending(['earlier', 'first', 'later']);
assert.deepEqual(ordered.drain('seen'), [{name:'earlier'}, {name:'first'}]);
assert.deepEqual(ordered.drain('later'), []);
// Host output consumers obey the same ordering, including reentrant events.
// A failing consumer must not remove later channels before they are processed.
const channels = ['earlier', 'first', 'later'];
const seen = [];
ordered.emit('first', {number:1});
ordered.consumePending(channels, (index, events) => {
  seen.push([channels[index], events]);
  if (index === 1) {
    ordered.emit('earlier', {next:true});
    ordered.emit('later', {same:true});
  }
});
assert.deepEqual(seen, [['first', [{number:1}]], ['later', [{same:true}]]]);
assert.deepEqual(ordered.drain('earlier'), [{next:true}]);
ordered.emit('first', {number:2}); ordered.emit('later', {keep:true});
assert.throws(() => ordered.consumePending(channels, () => { throw new Error('consumer failed'); }), /consumer failed/);
assert.deepEqual(ordered.drain('later'), [{keep:true}]);
// Fixed channel lists use the same ordered, reentrant and failure semantics.
const inputNames = ['earlier', 'first', 'later'];
const fixedChannels = new EventChannels(inputNames);
inputNames[1] = 'changed';
assert.deepEqual(fixedChannels.names, ['earlier', 'first', 'later']);
assert.throws(() => fixedChannels.names.push('other'), TypeError);
assert.throws(() => new EventChannels(['ok', 1]), TypeError);
assert.throws(() => new EventChannels(new Array(1)), TypeError);
ordered.emit('first', {number:3});
const fixedSeen = [];
ordered.consumePending(fixedChannels, (index, events) => {
  fixedSeen.push([fixedChannels.names[index], events]);
  if (index === 1) ordered.emit('later', {same:true});
});
assert.deepEqual(fixedSeen, [['first', [{number:3}]], ['later', [{same:true}]]]);
ordered.emit('first', {number:4}); ordered.emit('later', {keep:true});
assert.throws(() => ordered.consumePending(fixedChannels, () => { throw new Error('fixed failure'); }), /fixed failure/);
assert.deepEqual(ordered.drain('later'), [{keep:true}]);
ordered.emit('first', {initial:true});
ordered.dispatchPending(fixedChannels);
assert.deepEqual(ordered.drain('seen'), [{name:'later'}]);
ordered.dispatchPending(fixedChannels);
assert.deepEqual(ordered.drain('seen'), [{name:'earlier'}, {name:'first'}]);
const profiledChannels = new LuaScene(null, orderedWorld, 'gameplay', true);
profiledChannels.consumePending(fixedChannels, () => {});
assert.equal(profiledChannels.profileReport().operations.drain_next.count, 1);
ordered.close();

// Time spent inside an ECS host call must not exhaust a healthy Lua tick.
const stallSignal = new Int32Array(new SharedArrayBuffer(4));
const stalledWorld = new WorldFixture({});
const originalOperation = stalledWorld.ecs_operation.bind(stalledWorld);
stalledWorld.ecs_operation = (...args) => {
  Atomics.wait(stallSignal, 0, 0, 80);
  return originalOperation(...args);
};
const stalled = await LuaScene.create(factory, stalledWorld, 'gameplay', engineApi, [
  "ecs.system('stalled', function() ecs.get_resource('missing'); local n=0; for i=1,100000 do n=n+i end end)"
]);
assert.doesNotThrow(() => stalled.run(1/60));
stalled.close();

// A worker deadline ensures a broken budget cannot hang the test process.
const budgetWorker = new Worker(new URL('./lua_budget_worker.mjs', import.meta.url));
let budgetTimer;
try {
  const results = await new Promise((resolve, reject) => {
    budgetTimer = setTimeout(() => reject(new Error('Lua budget worker timed out')), 5000);
    budgetWorker.once('message', resolve);
    budgetWorker.once('error', reject);
  });
  assert.equal(results.length, 4);
  for (const result of results) assert.match(result, /Lua instruction budget exceeded/);
} finally {
  clearTimeout(budgetTimer);
  await budgetWorker.terminate();
}

// Optional profiling must measure real Lua systems and leave its host callbacks private.
const profiledWorld = new WorldFixture({});
const profiled = await LuaScene.create(factory, profiledWorld, 'gameplay', engineApi, [
  "assert(__profile_now == nil and __profile_sample == nil); ecs.system('example', function() ecs.get_resource('missing') end)"
], true);
profiled.run(1/60);
assert.equal(profiled.profileReport().systems.example.count, 1);
assert.equal(profiled.profileReport().operations.get_resource.count, 1);
profiled.close();

const menuWorld = new WorldFixture({resume:'resume-id', levels:'levels-id'});
const pause = await LuaScene.create(factory, menuWorld, 'menu', engineApi, [menu]);
pause.emit('input.key', {key:'Escape', pressed:true, is_repeat:false});
pause.run(0);
assert.deepEqual(pause.drain('engine.scene'), [{action:'pop'}]);
pause.emit('ui.action', {id:'resume-id', kind:{type:'clicked'}});
pause.run(0);
assert.deepEqual(pause.drain('engine.scene'), [{action:'pop'}]);
pause.emit('ui.action', {id:'levels-id', kind:{type:'clicked'}});
pause.run(0);
assert.deepEqual(pause.drain('engine.scene'), [{action:'replace', scene:'level_selector'}]);
pause.close();

// Scene startup budgets are independent: earlier scripts must not consume the
// time available to later ones, and a runaway script must still be stopped.
const originalNow = Date.now;
let clock = 1000, deadline = null, starts = 0;
try {
  Date.now = () => clock;
  const fakeVm = {
    global: {
      setMemoryMax() {}, set() {}, close() {},
      setTimeout(value) { deadline = value; },
      get() { return () => {}; },
    },
    doStringSync() {
      assert.equal(deadline - clock, 500);
      starts++;
      clock += 200;
    },
  };
  const fake = await LuaScene.create({async createEngine() { return fakeVm; }}, world, 'gameplay', 'engine', ['one', 'two']);
  assert.equal(starts, 4);
  assert.equal(deadline, undefined);
  fake.close();
} finally { Date.now = originalNow; }
await assert.rejects(
  LuaScene.create(factory, world, 'gameplay', engineApi, ['while true do end']),
  /thread timeout exceeded/
);
console.log('Browser Lua scene, execution limits, and ECS bridge tests passed.');
