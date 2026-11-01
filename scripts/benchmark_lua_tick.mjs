import {performance} from 'node:perf_hooks';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require = createRequire(import.meta.url);
const {LuaFactory} = require('../third_party/wasmoon/index.js');
const target = {ecs_operation() { return '{"found":true,"value":{"x":1,"y":2}}'; }};
const scene = await LuaScene.create(new LuaFactory(), target, 'gameplay', '', [
  "ecs.system('tick', function(dt) local n=0; for i=1,200 do n=n+i*dt end; local value=ecs.get_resource('value'); assert(value.x==1) end)"
]);
const count=2000;
function round() {
  const start=performance.now();
  for (let i=0;i<count;i++) scene.run(1/60);
  return performance.now()-start;
}
for(let i=0;i<3;i++) round();
const samples=Array.from({length:7},round).sort((a,b)=>a-b);
console.log(JSON.stringify({fixture:'2,000 Lua ticks with one ECS read', median_ms:samples[3], samples_ms:samples}, null, 2));
scene.close();
