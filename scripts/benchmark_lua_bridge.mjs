import { performance } from 'node:perf_hooks';
import { createRequire } from 'node:module';
import { LuaScene } from '../preview/lua_runtime.js';

const require = createRequire(import.meta.url);
const { LuaFactory } = require('../third_party/wasmoon/index.js');

// Keep the JSON transport in the measurement; this fixture intentionally does
// not measure Rust or rendering. Use the in-browser profiler for end-to-end runs.
const body = {
  position: [1.2, 1.8, -3.4], velocity: [0, -0.1, 0], enabled: true,
  collision: true, grounded: true, mass: 80, gravity: 9.81,
  radius: 0.35, height: 1.8, mode: 'character', metadata: { tag: 'player' },
};
const target = {
  ecs_operation(_layer, encoded) {
    const request = JSON.parse(encoded);
    if (request.op !== 'get') throw new Error('Unexpected operation');
    return JSON.stringify({ found: true, value: body });
  }
};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', []);
const loop = "for i = 1, 3000 do local body = ecs.get('1:0:0', 'PhysicsBody'); assert(body.position[2] == 1.8 and body.metadata.tag == 'player') end";
function round() {
  scene.vm.global.setTimeout(Date.now() + 30000);
  const start = performance.now();
  try { scene.vm.doStringSync(loop); }
  finally { scene.vm.global.setTimeout(undefined); }
  return performance.now() - start;
}
for (let i = 0; i < 3; i++) round();
const samples = Array.from({length: 9}, round).sort((a, b) => a - b);
console.log(JSON.stringify({kind:'browser Lua fixture (3,000 nested component reads)', median_ms:samples[4], min_ms:samples[0], max_ms:samples[8], samples_ms:samples}, null, 2));
scene.close();
