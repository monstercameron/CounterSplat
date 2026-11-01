// Exercise the actual browser Lua 5.4 runtime and project scripts without a GPU.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
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
  constructor(ids) { this.resources = new Map([['scene.entities', ids]]); this.events = new Map(); }
  ecs_operation(_layer, encoded) {
    const op = JSON.parse(encoded);
    let value, found = true;
    switch (op.op) {
      case 'hierarchy': value = [{id:'root', parent:null}, {id:'child', parent:'root'}]; break;
      case 'get_resource': found = this.resources.has(op.name); value = this.resources.get(op.name); break;
      case 'get_resource_field': {
        const resource = this.resources.get(op.name);
        found = resource !== null && typeof resource === 'object' && Object.hasOwn(resource, op.field);
        value = found ? resource[op.field] : null; break;
      }
      case 'set_resource': this.resources.set(op.name, op.value); found = false; break;
      case 'emit': this.events.set(op.name, [...(this.events.get(op.name) || []), op.value]); found = false; break;
      case 'drain': value = this.events.get(op.name) || []; this.events.delete(op.name); break;
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
world.resources.set('special-keys', JSON.parse('{"__proto__":{"items":[{},[],null]},"constructor":"safe","kind":"user","entries":"data"}'));
game.vm.doStringSync(`
  local special = ecs.get_resource('special-keys')
  assert(special.__proto__.items[1] ~= nil and #special.__proto__.items[2] == 0)
  assert(special.__proto__.items[3] == ecs.null)
  assert(special.constructor == 'safe' and special.kind == 'user' and special.entries == 'data')
`);
game.vm.doStringSync(`
  ecs.set_resource('roundtrip', {items = ecs.array(), marker = ecs.null, nested = {1, 2}})
  local tree = ecs.hierarchy()
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
game.close();

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
