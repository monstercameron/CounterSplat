import {performance} from 'node:perf_hooks';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
// JSON and Lua/JS conversion are included; Rust, rendering and editor writes are not.
const lens={aperture_f_number:5.6,shutter_angle_degrees:180,shutter_fps:60,iso:400,
  exposure_compensation_ev:0,focus_distance_m:12,sensor_height_mm:24,depth_of_field:true,motion_blur:false};
const target={ecs_operation(_layer, raw) {
  const op=JSON.parse(raw);
  if(op.op==='get') return JSON.stringify({found:true,value:lens});
  if(op.op==='get_if_changed') {
    const revision=op.name+'-revision';
    return JSON.stringify(op.revision===revision ? {found:false,value:null} :
      {found:true,value:{revision,present:true,value:lens}});
  }
  throw new Error('Unexpected operation');
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay','',[]);
const loops={
  full:"for i=1,1500 do local a=ecs.get('entity','CameraLens'); local b=ecs.get('entity','CameraZoomLens'); assert(a.iso==400 and b.iso==400) end",
  cached:"local a='CameraLens-revision';local b='CameraZoomLens-revision';for i=1,1500 do assert(ecs.get_if_changed('entity','CameraLens',a)==nil and ecs.get_if_changed('entity','CameraZoomLens',b)==nil) end",
};
function round(source) {
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try {scene.vm.doStringSync(source);} finally {scene.vm.global.setTimeout(undefined);}
  return performance.now()-start;
}
for(let i=0;i<3;i++){round(loops.full);round(loops.cached);}
const samples={full:[],cached:[]};
for(let i=0;i<11;i++) for(const key of (i%2?['cached','full']:['full','cached']))samples[key].push(round(loops[key]));
for(const values of Object.values(samples))values.sort((a,b)=>a-b);
console.log(JSON.stringify({iterations:1500,full_ms:samples.full[5],cached_ms:samples.cached[5]}));
scene.close();
