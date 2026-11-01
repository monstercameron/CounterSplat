// Build the ECS Wasm fixture first; see docs/performance.md.
import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import {readFileSync} from 'node:fs';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
import {initSync, BenchWorld} from '../.artifacts/ecs-wasm-bench/pkg/ecs_bench.js';
const require = createRequire(import.meta.url), {LuaFactory} = require('../third_party/wasmoon/index.js');
initSync({module:readFileSync(new URL('../.artifacts/ecs-wasm-bench/pkg/ecs_bench_bg.wasm',import.meta.url))});
const world = new BenchWorld(), id = world.id();
function compare(request, direct) {
  const original = JSON.parse(world.ecs_operation('gameplay',JSON.stringify(request)));
  const raw = direct();
  assert.equal(raw != null, original.found);
  if (raw != null) assert.deepEqual(JSON.parse(raw), original.value);
}
for (const name of ['nullable','player.spawn_probe_pending','missing'])
  compare({op:'get_resource',name}, () => world.ecs_get_resource('gameplay',name));
for (const field of ['player','missing'])
  compare({op:'get_resource_field',name:'scene.entities',field}, () => world.ecs_get_resource_field('gameplay','scene.entities',field));
const target = {ecs_operation:(...args)=>world.ecs_operation(...args),
  ecs_get_resource:undefined, ecs_get_resource_field:undefined};
const scene = await LuaScene.create(new LuaFactory(),target,'gameplay','',[]);
scene.vm.global.set('bench_id',id);
for (const [label,source] of [
  ['5,000 resource reads', "for i=1,5000 do assert(ecs.get_resource('player.spawn_probe_pending')==false) end"],
  ['5,000 resource-field reads', "for i=1,5000 do assert(ecs.get_resource_field('scene.entities','player')==bench_id) end"]]) {
  const samples=[[],[]];
  for (let round=0; round<18; round++) for (const mode of (round%2 ? [1,0] : [0,1])) {
    target.ecs_get_resource = mode ? (...args)=>world.ecs_get_resource(...args) : undefined;
    target.ecs_get_resource_field = mode ? (...args)=>world.ecs_get_resource_field(...args) : undefined;
    scene.vm.global.setTimeout(Date.now()+30000); const start=performance.now();
    try { scene.vm.doStringSync(source); } finally { scene.vm.global.setTimeout(undefined); }
    if (round>=3) samples[mode].push(performance.now()-start);
  }
  for (const sample of samples) sample.sort((a,b)=>a-b);
  console.log(`${label}: Lua JSON ${samples[0][7].toFixed(2)} ms, direct ${samples[1][7].toFixed(2)} ms (median of 15 alternating samples)`);
}
scene.close(); world.free();
