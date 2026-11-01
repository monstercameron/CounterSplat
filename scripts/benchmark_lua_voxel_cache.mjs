import {performance} from 'node:perf_hooks';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';
const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
const engineApi=await readFile(new URL('../crates/scripting-lua/src/engine_api.lua',import.meta.url),'utf8');
const values={VoxelCollider:{enabled:true,visible:false,physics:true,player_radius:.2},
 VoxelColliderState:{status:'ready',resolution:.05,visible_voxels:0,message:''}};
const target={ecs_operation(_layer,raw){
 const op=JSON.parse(raw);
 if(op.op==='get')return JSON.stringify({found:true,value:values[op.name]});
 if(op.op==='get_if_changed'){
  const revision=op.name+'-revision';
  return JSON.stringify(op.revision===revision?{found:false,value:null}:
    {found:true,value:{revision,present:true,value:values[op.name]}});
 }
 throw new Error('unexpected');
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay',engineApi,[]);
const loops={
 full:"for i=1,1500 do local a=ecs.get('level','VoxelCollider'); local b=ecs.get('level','VoxelColliderState'); assert(a.enabled and b.status=='ready') end",
 cached:"local a=engine.component_reader('level','VoxelCollider'); local b=engine.component_reader('level','VoxelColliderState'); for i=1,1500 do assert(a().enabled and b().status=='ready') end",
};
function round(source){scene.vm.global.setTimeout(Date.now()+30000);const start=performance.now();try{scene.vm.doStringSync(source)}finally{scene.vm.global.setTimeout(undefined)}return performance.now()-start}
for(let i=0;i<3;i++){round(loops.full);round(loops.cached)}
const samples={full:[],cached:[]};
for(let i=0;i<11;i++)for(const k of (i%2?['cached','full']:['full','cached']))samples[k].push(round(loops[k]));
for(const a of Object.values(samples))a.sort((a,b)=>a-b);
console.log(JSON.stringify({iterations:1500,full_ms:samples.full[5],cached_ms:samples.cached[5]}));scene.close();
