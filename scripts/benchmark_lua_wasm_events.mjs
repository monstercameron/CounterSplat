// Build the ECS Wasm fixture first; see docs/performance.md.
import {performance} from 'node:perf_hooks';
import {readFileSync} from 'node:fs';
import {LuaScene,EventChannels} from '../preview/lua_runtime.js';
import {GAMEPLAY_INPUT_CHANNELS,GAMEPLAY_EVENT_CHANNELS} from '../preview/gameplay_events.js';
import {initSync,BenchWorld} from '../.artifacts/ecs-wasm-bench/pkg/ecs_bench.js';
initSync({module:readFileSync(new URL('../.artifacts/ecs-wasm-bench/pkg/ecs_bench_bg.wasm',import.meta.url))});
const world=new BenchWorld(), scene=new LuaScene(null,world,'gameplay');
const all=new EventChannels([...GAMEPLAY_INPUT_CHANNELS,...GAMEPLAY_EVENT_CHANNELS]);
const early=new EventChannels(GAMEPLAY_INPUT_CHANNELS), late=new EventChannels(GAMEPLAY_EVENT_CHANNELS);
const samples=[[],[]], count=20000;
for(let round=0;round<13;round++) for(const mode of (round%2 ? [1,0] : [0,1])) {
  const start=performance.now();
  for(let i=0;i<count;i++) {
    if(mode) { scene.dispatchPending(early); scene.dispatchPending(late); }
    else scene.dispatchPending(all);
  }
  if(round>=2) samples[mode].push((performance.now()-start)*1000/count);
}
for(const sample of samples) sample.sort((a,b)=>a-b);
console.log(`Empty event pass: unsplit ${samples[0][5].toFixed(3)} µs, split ${samples[1][5].toFixed(3)} µs (median of 11 alternating samples)`);
world.free();
