// Build the ECS Wasm fixture first; see docs/performance.md.
import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import {readFileSync} from 'node:fs';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
import {initSync, BenchWorld} from '../.artifacts/ecs-wasm-bench/pkg/ecs_bench.js';
const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
initSync({module:readFileSync(new URL('../.artifacts/ecs-wasm-bench/pkg/ecs_bench_bg.wasm', import.meta.url))});
const world = new BenchWorld(), id = world.id(), originalRevision = world.revision();
function check(revision) {
  const request = {op:'get_if_changed',id,name:'PlayerControls',revision};
  const json = JSON.parse(world.ecs_operation('gameplay',JSON.stringify(request)));
  const direct = world.ecs_get_if_changed('gameplay',id,'PlayerControls',revision);
  assert.deepEqual(direct == null ? null : JSON.parse(direct), json.found ? json.value : null);
}
check(originalRevision); check('stale');
world.set_speed(5); check(originalRevision);
assert.throws(() => world.ecs_get_if_changed('gameplay','bad','PlayerControls','stale'));
const currentRevision = world.revision();
let directCalls = 0;
const direct = (...args) => { directCalls++; return world.ecs_get_if_changed(...args); };
const target = {ecs_operation:(...args) => world.ecs_operation(...args), ecs_get_if_changed:undefined};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', []);
scene.vm.global.set('bench_id',id); scene.vm.global.set('bench_revision',currentRevision);
const source = "for i=1,5000 do assert(ecs.get_if_changed(bench_id,'PlayerControls',bench_revision)==nil) end";
const samples = [[],[]];
for (let round=0; round<24; round++) for (const mode of (round%2 ? [1,0] : [0,1])) {
  target.ecs_get_if_changed = mode ? direct : undefined;
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try { scene.vm.doStringSync(source); } finally { scene.vm.global.setTimeout(undefined); }
  if (round>=3) samples[mode].push(performance.now()-start);
}
for (const sample of samples) sample.sort((a,b)=>a-b);
if (!directCalls) throw new Error('Direct read path was not exercised');
console.log(`Lua JSON ${samples[0][10].toFixed(2)} ms, direct ${samples[1][10].toFixed(2)} ms per 5000 unchanged reads (median of 21 alternating samples)`);
scene.close(); world.free();
