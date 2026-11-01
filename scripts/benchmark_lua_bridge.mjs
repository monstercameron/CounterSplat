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
    if (request.op === 'get') return JSON.stringify({ found: true, value: body });
    if (request.op === 'is_alive') return JSON.stringify({ found: true, value: true });
    if (request.op === 'get_resource_field') return JSON.stringify({ found: true, value: '1:0:0' });
    throw new Error('Unexpected operation');
  }
};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', []);
const cases = {
  nested: "for i = 1, 3000 do local body = ecs.get('1:0:0', 'PhysicsBody'); assert(body.position[2] == 1.8 and body.metadata.tag == 'player') end",
  scalar: "for i = 1, 3000 do assert(ecs.is_alive('1:0:0')); assert(ecs.get_resource_field('scene.entities','player') == '1:0:0') end",
};
for (const [kind, loop] of Object.entries(cases)) {
  function round() {
    scene.vm.global.setTimeout(Date.now() + 30000);
    const start = performance.now();
    try { scene.vm.doStringSync(loop); }
    finally { scene.vm.global.setTimeout(undefined); }
    return performance.now() - start;
  }
  for (let i = 0; i < 3; i++) round();
  const samples = Array.from({length: 9}, round).sort((a, b) => a - b);
  console.log(JSON.stringify({kind, iterations:3000, median_ms:samples[4], min_ms:samples[0], max_ms:samples[8]}));
}
scene.close();
