import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
const api=await readFile(new URL('../crates/scripting-lua/src/engine_api.lua',import.meta.url),'utf8');
const body={enabled:true,collision_enabled:true,gravity_enabled:false,gravity:18,
  grounded:true,velocity:[0,0,0],radius:.25,capsule_half_height:.7,collision_offset_y:-.8};
const target={ecs_operation(_layer,raw){
  const op=JSON.parse(raw);
  if(op.op==='get')return JSON.stringify({found:true,value:body});
  if(op.op==='is_alive' || op.op==='has')return JSON.stringify({found:true,value:true});
  if(op.op==='query')return JSON.stringify({found:true,value:['player']});
  throw new Error('Unexpected operation '+op.op);
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay',api,[]);
const loops={
  duplicate:"for i=1,1000 do local body=ecs.get('player','PhysicsBody'); assert(body.enabled); engine.physics.step(.016) end",
  predicate:"local function skip(id,body) assert(body.enabled); return false end; for i=1,1000 do assert(ecs.is_alive('player')); engine.physics.step(.016,skip) end",
};
function round(source){scene.vm.global.setTimeout(Date.now()+30000);const start=performance.now();
 try{scene.vm.doStringSync(source)}finally{scene.vm.global.setTimeout(undefined)}return performance.now()-start}
for(let i=0;i<3;i++){round(loops.duplicate);round(loops.predicate)}
const samples={duplicate:[],predicate:[]};
for(let i=0;i<11;i++)for(const key of (i%2?['predicate','duplicate']:['duplicate','predicate']))samples[key].push(round(loops[key]));
for(const sample of Object.values(samples))sample.sort((a,b)=>a-b);
console.log(JSON.stringify({ticks:1000,duplicate_ms:samples.duplicate[5],predicate_ms:samples.predicate[5]}));scene.close();
