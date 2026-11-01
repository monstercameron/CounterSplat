import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
const engineApi=await readFile(new URL('../crates/scripting-lua/src/engine_api.lua',import.meta.url),'utf8');
// Gameplay-shaped snapshots; this measures Lua/JS transport, not Rust or GPU work.
const values={
  PlayerCamera:{position:[1.9,1.6,-2.4],view_offset:[0,0,0],view_posture_offset_y:0,
    view_bob_pitch:0,view_bob_roll:0,view_turn_roll:0,yaw:-2,pitch:0,roll:0,fov_degrees:71,view_fov_degrees:null},
  PlayerRig:{eye_height_m:1.7,head_clearance_m:.15,radius_m:.25,units_per_meter:1,auto_align_spawn:true},
  PlayerControls:{enabled:true,mouse_look:true,move_speed:4,sprint_multiplier:3,mouse_sensitivity:.003,
    jump_speed:5,jump_gravity:18,fov_degrees:null,invert_y:false,bob_enabled:true,bob_cadence:2,
    bob_vertical:.035,bob_horizontal:.018,bob_smoothing:12,zoom_factor:5,zoom_response:12,
    bindings:{forward:'KeyW',backward:'KeyS',left:'KeyA',right:'KeyD',up:'KeyE',down:'KeyQ',
      jump:'Space',sprint:'ShiftLeft',crouch:'ControlLeft'}},
};
const target={ecs_operation(_layer,raw){
  const op=JSON.parse(raw);
  if(op.op==='get') return JSON.stringify({found:true,value:values[op.name]});
  if(op.op==='get_if_changed') {
    const revision=op.name+'-revision';
    return JSON.stringify(op.revision===revision ? {found:false,value:null} :
      {found:true,value:{revision,present:true,value:values[op.name]}});
  }
  if(op.op==='get_many') {
    const value=Object.create(null);
    for(const name of op.names)value[name]=values[name];
    return JSON.stringify({found:true,value});
  }
  throw new Error('Unexpected operation '+op.op);
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay',engineApi,[]);
const loops={
  batch:"for i=1,1000 do local v=ecs.get_many('entity',{'PlayerCamera','PlayerControls','PlayerRig'}); assert(v.PlayerCamera.fov_degrees==71 and v.PlayerControls.move_speed==4 and v.PlayerRig.units_per_meter==1) end",
  cached:"local controls=engine.component_reader('entity','PlayerControls'); local rig=engine.component_reader('entity','PlayerRig'); for i=1,1000 do local c=ecs.get('entity','PlayerCamera'); assert(c.fov_degrees==71 and controls().move_speed==4 and rig().units_per_meter==1) end",
};
function round(source){
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try {scene.vm.doStringSync(source);} finally {scene.vm.global.setTimeout(undefined);}
  return performance.now()-start;
}
for(let i=0;i<3;i++){round(loops.batch);round(loops.cached);}
const samples={batch:[],cached:[]};
for(let i=0;i<11;i++)for(const key of (i%2?['cached','batch']:['batch','cached']))samples[key].push(round(loops[key]));
for(const sample of Object.values(samples))sample.sort((a,b)=>a-b);
console.log(JSON.stringify({iterations:1000,batch_ms:samples.batch[5],cached_ms:samples.cached[5]}));
scene.close();
