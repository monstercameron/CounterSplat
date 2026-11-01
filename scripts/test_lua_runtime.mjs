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
      case 'get_resource': found = this.resources.has(op.name); value = this.resources.get(op.name); break;
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
game.vm.doStringSync(`
  ecs.set_resource('roundtrip', {items = ecs.array(), marker = ecs.null, nested = {1, 2}})
  local value = ecs.get_resource('roundtrip')
  assert(#value.items == 0 and value.marker == ecs.null and value.nested[2] == 2)
  ecs.set_resource('again', value)
  assert(not pcall(function() ecs.set_resource('bad', 0/0) end))
  local cycle = {}; cycle.self = cycle
  assert(not pcall(function() ecs.set_resource('bad', cycle) end))
`);
assert.deepEqual(world.resources.get('again'), {items:[], marker:null, nested:[1, 2]});
game.close();

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
console.log('Browser Lua scene and ECS bridge tests passed.');
