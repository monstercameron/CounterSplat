import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
const engineApi=await readFile(new URL('../crates/scripting-lua/src/engine_api.lua',import.meta.url),'utf8');
const manifest=JSON.parse(await readFile(new URL('../assets/scenes/gameplay.json',import.meta.url),'utf8'));
const values=manifest.entities.find(e=>e.id==='first_person').components;
const target={ecs_operation(_layer,raw){
  const op=JSON.parse(raw);
  if(op.op==='get') return JSON.stringify({found:true,value:values[op.name]});
  if(op.op==='get_if_changed'){
    const revision=op.name+'-revision';
    return JSON.stringify(op.revision===revision ? {found:false,value:null} :
      {found:true,value:{revision,present:true,value:values[op.name]}});
  }
  throw new Error('Unexpected operation '+op.op);
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay',engineApi,[]);
const loops={
  full:"for i=1,1000 do local c=ecs.get('weapon','WeaponConfig'); local a=ecs.get('weapon','WeaponAnimation'); local v=ecs.get('weapon','FirstPersonView'); assert(c.magazine_capacity==30 and a.idle_enabled and v.enabled) end",
  cached:"local c=engine.component_reader('weapon','WeaponConfig'); local a=engine.component_reader('weapon','WeaponAnimation'); local v=engine.component_reader('weapon','FirstPersonView'); for i=1,1000 do assert(c().magazine_capacity==30 and a().idle_enabled and v().enabled) end",
};
function round(source){
  scene.vm.global.setTimeout(Date.now()+30000);
  const start=performance.now();
  try {scene.vm.doStringSync(source);} finally {scene.vm.global.setTimeout(undefined);}
  return performance.now()-start;
}
for(let i=0;i<3;i++){round(loops.full);round(loops.cached);}
const samples={full:[],cached:[]};
for(let i=0;i<11;i++)for(const key of (i%2?['cached','full']:['full','cached']))samples[key].push(round(loops[key]));
for(const sample of Object.values(samples))sample.sort((a,b)=>a-b);
console.log(JSON.stringify({iterations:1000,full_ms:samples.full[5],cached_ms:samples.cached[5]}));
scene.close();
