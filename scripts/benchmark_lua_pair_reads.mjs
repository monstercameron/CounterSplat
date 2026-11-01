import {performance} from 'node:perf_hooks';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const values = {
  PlayerCamera:{position:[1,2,3], yaw:-2, pitch:0, view_offset:[0,0,0], fov_degrees:85},
  PhysicsBody:{enabled:true, collision_enabled:true, gravity_enabled:true, grounded:true, velocity:[0,0,0], radius:0.25}
};
const target = {ecs_operation(_layer, raw) {
  const op = JSON.parse(raw);
  if (op.op === 'get') return JSON.stringify({found:true, value:values[op.name]});
  if (op.op === 'get_many') {
    const value = Object.create(null);
    for (const name of op.names) if (Object.hasOwn(values, name)) value[name] = values[name];
    return JSON.stringify({found:true, value});
  }
  throw new Error('Unexpected operation');
}};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', []);
const sources = {
  separate:"for i=1,2000 do local c=ecs.get('entity','PlayerCamera'); local b=ecs.get('entity','PhysicsBody'); assert(c.position[2]==2 and b.grounded) end",
  batch:"for i=1,2000 do local p=ecs.get_many('entity',{'PlayerCamera','PhysicsBody'}); assert(p.PlayerCamera.position[2]==2 and p.PhysicsBody.grounded) end"
};
function run(source) {
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try { scene.vm.doStringSync(source); } finally { scene.vm.global.setTimeout(undefined); }
  return performance.now()-start;
}
for (let i=0;i<3;i++) { run(sources.separate); run(sources.batch); }
const samples={separate:[],batch:[]};
for (let i=0;i<11;i++) for (const key of (i%2 ? ['batch','separate'] : ['separate','batch']))
  samples[key].push(run(sources[key]));
for (const sample of Object.values(samples)) sample.sort((a,b)=>a-b);
console.log(JSON.stringify({pairs:2000,separate_ms:samples.separate[5],batch_ms:samples.batch[5]}));
scene.close();
