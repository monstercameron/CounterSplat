import {performance} from 'node:perf_hooks';
import {LuaScene, EventChannels} from '../preview/lua_runtime.js';

// Isolate host-side JSON transport overhead. The fixture supplies no events;
// the browser profiler measures the actual Rust/WASM and gameplay paths.
const target = {
  ecs_operation(_layer, encoded) {
    const op = JSON.parse(encoded);
    if (op.op === 'drain') return JSON.stringify({found:true, value:[]});
    if (op.op === 'drain_next') return JSON.stringify({found:false, value:null});
    throw new Error('Unexpected operation');
  }
};
const channels = ['input.key', 'input.mouse_delta', 'input.mouse_button', 'input.reset',
  'ui.action', 'editor.entity_selected', 'physics.ready', 'physics.ground_result',
  'physics.clearance_result', 'physics.trace_result', 'physics.motion_resolved',
  'physics.motion_error', 'physics.collision', 'physics.error', 'weapon.trigger',
  'weapon.aim', 'weapon.reload', 'weapon.equip', 'weapon.mode', 'weapon.hit',
  'weapon.die', 'weapon.posture'];
const scene = new LuaScene(null, target, 'gameplay');
const fixed = new EventChannels(channels);
const count = 10000;
function sample(fn) {
  for (let i = 0; i < 1000; i++) fn();
  const times = [];
  for (let round = 0; round < 7; round++) {
    const start = performance.now();
    for (let i = 0; i < count; i++) fn();
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  return times[3];
}
const separate = sample(() => { for (const name of channels) scene.drain(name); });
const batched = sample(() => scene.dispatchPending(channels));
const preencoded = sample(() => scene.dispatchPending(fixed));
console.log(JSON.stringify({fixture:'10,000 empty dispatch passes', separate_ms:separate, batched_ms:batched, preencoded_ms:preencoded}, null, 2));
