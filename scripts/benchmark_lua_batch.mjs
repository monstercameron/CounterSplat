import {performance} from 'node:perf_hooks';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
const values = {
  Posture: {amount:0, moving:false, crouched:false, blocked:false},
  Controls: {enabled:true, move_speed:4, sprint_multiplier:1.5, bindings:{forward:'KeyW', sprint:'ShiftLeft'}},
  Body: {position:[1,2,3], velocity:[0,0,0], enabled:true, grounded:true, gravity:9.81, radius:0.35},
};
const target = {
  ecs_operation(_layer, request) {
    const op=JSON.parse(request);
    if (op.op==='get') return JSON.stringify({found:true,value:values[op.name]});
    if (op.op==='get_many') {
      const result=Object.create(null);
      for (const name of op.names) if (Object.hasOwn(values,name)) result[name]=values[name];
      return JSON.stringify({found:true,value:result});
    }
    throw new Error('Unexpected operation');
  }
};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay','',[]);
const loops={
  separate:"for i=1,2000 do local a=ecs.get('1:0.0','Posture'); local b=ecs.get('1:0.0','Controls'); local c=ecs.get('1:0.0','Body'); assert(a.amount==0 and b.enabled and c.grounded) end",
  batched:"for i=1,2000 do local v=ecs.get_many('1:0.0',{'Posture','Controls','Body'}); assert(v.Posture.amount==0 and v.Controls.enabled and v.Body.grounded) end",
};
function measure(source) {
  function round() {
    scene.vm.global.setTimeout(Date.now()+30000);
    const start=performance.now();
    try {scene.vm.doStringSync(source);}
    finally {scene.vm.global.setTimeout(undefined);}
    return performance.now()-start;
  }
  for(let i=0;i<2;i++) round();
  const samples=Array.from({length:7},round).sort((a,b)=>a-b);
  return samples[3];
}
const results={fixture:'2,000 three-component reads',separate_ms:measure(loops.separate)};
if (process.argv.includes('--batch')) results.batched_ms=measure(loops.batched);
console.log(JSON.stringify(results,null,2));
scene.close();
