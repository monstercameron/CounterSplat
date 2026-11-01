// Exercise the actual browser Lua 5.4 runtime and project scripts without a GPU.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {Worker} from 'node:worker_threads';
import {LuaScene} from '../preview/lua_runtime.js';

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
        const components = this.components.get(op.id) || new Map();
        const revision = `${op.id}|${op.name}|${this.revision}`;
        if (op.revision === revision) { found = false; break; }
        value = {revision, present:components.has(op.name), value:components.get(op.name) ?? null};
        break;
      }
      case 'get_many': {
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
      case 'emit': this.events.set(op.name, [...(this.events.get(op.name) || []), op.value]); found = false; break;
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
world.resources.set('special-keys', JSON.parse('{"__proto__":{"items":[{},[],null]},"constructor":"safe","kind":"user","entries":"data"}'));
world.resources.set('nested-roundtrip', JSON.parse('{"rows":[{"name":"original","enabled":false},[],null],"empty":{},"empty_list":[],"zero":0}'));
game.vm.doStringSync(`
  local count = select('#', pcall(function() return 1, nil, 3 end))
  assert(count == 4)
  local ok, value = xpcall(function() error('ordinary') end, function() return 'handled' end)
  assert(not ok and value == 'handled')
  assert(not pcall(function() xpcall(function() end, nil) end))
  assert(__ecs_call == nil and __ecs_call_flat == nil)
  assert(ecs.is_alive('entity') and not ecs.is_alive('missing'))
  assert(ecs.get('entity', 'Health') == 50 and ecs.has('entity', 'Health'))
  assert(ecs.get('entity', 'Nullable') == ecs.null and ecs.get('entity', 'Missing') == nil)
  assert(ecs.get('entity', '__proto__') == 'safe' and not ecs.has('entity', 'Missing'))
  ecs.set_field('entity', 'Stats', 'hp', 25)
  ecs.set_field('entity', 'Stats', 'optional', ecs.null)
  ecs.set_field('entity', 'Stats', '__proto__', 'safe')
  ecs.set_fields('entity', 'Stats', {hp=30, optional=ecs.null})
  ecs.set_fields('entity', 'Stats', {})
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', ecs.array()) end))
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', {hp=0/0}) end))
  local patch_cycle = {}; patch_cycle.self = patch_cycle
  assert(not pcall(function() ecs.set_fields('entity', 'Stats', patch_cycle) end))
  local stats = ecs.get('entity', 'Stats')
  assert(stats.hp == 30 and stats.keep and stats.optional == ecs.null and stats.__proto__ == 'safe')
  local changed = ecs.get_if_changed('entity', 'Stats')
  assert(changed.present and changed.value.hp == 30 and type(changed.revision) == 'string')
  assert(ecs.get_if_changed('entity', 'Stats', changed.revision) == nil)
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
  assert(not pcall(function() ecs.set_field('entity', 'Missing', 'hp', 1) end))
  assert(not pcall(function() ecs.set_field('entity', 'Stats', 'bad', 0/0) end))
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
  {op:'remove', id:'entity', name:'Name'}, {op:'get_resource', name:'settings'},
  {op:'get_resource_field', name:'settings', field:'field'},
  {op:'remove_resource', name:'settings'}, {op:'parent', id:'entity'},
  {op:'children', id:'entity'}, {op:'drain', name:'event'}, {op:'snapshot'},
]);
scalar.close();

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
