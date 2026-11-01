// Build the ECS Wasm fixture first; see docs/performance.md.
import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import {readFileSync} from 'node:fs';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
import {initSync, BenchWorld} from '../.artifacts/ecs-wasm-bench/pkg/ecs_bench.js';
const require = createRequire(import.meta.url), {LuaFactory} = require('../third_party/wasmoon/index.js');
initSync({module:readFileSync(new URL('../.artifacts/ecs-wasm-bench/pkg/ecs_bench_bg.wasm', import.meta.url))});
const world = new BenchWorld(), id = world.id();
const target = {ecs_operation:(...args) => world.ecs_operation(...args),
  ecs_get_field:(...args) => world.ecs_get_field(...args)};
assert.equal(JSON.parse(world.ecs_get_field('gameplay',id,'PlayerControls','speed')), 4);
world.set_speed(4.2);
const full = JSON.parse(world.ecs_operation('gameplay',JSON.stringify({op:'get',id,name:'PlayerControls'}))).value;
assert.equal(JSON.parse(world.ecs_get_field('gameplay',id,'PlayerControls','speed')), full.speed);
world.set_speed(4);
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', []);
scene.vm.global.set('bench_id',id);
const sources = [
  "for i=1,5000 do local c=ecs.get(bench_id,'PlayerControls'); assert(c.speed==4) end",
  "for i=1,5000 do assert(ecs.get_field(bench_id,'PlayerControls','speed')==4) end"
];
const samples = [[],[]];
for (let round=0; round<24; round++) for (const mode of (round%2 ? [1,0] : [0,1])) {
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try { scene.vm.doStringSync(sources[mode]); } finally { scene.vm.global.setTimeout(undefined); }
  if (round>=3) samples[mode].push(performance.now()-start);
}
for (const sample of samples) sample.sort((a,b)=>a-b);
console.log(`5,000 component reads: full ${samples[0][10].toFixed(2)} ms, field ${samples[1][10].toFixed(2)} ms (median of 21 alternating samples)`);
scene.close(); world.free();
