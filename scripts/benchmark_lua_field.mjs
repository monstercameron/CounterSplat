import {performance} from 'node:perf_hooks';
import {createRequire} from 'node:module';
import {LuaScene} from '../preview/lua_runtime.js';

const require=createRequire(import.meta.url);
const {LuaFactory}=require('../third_party/wasmoon/index.js');
// This isolates Lua/JS serialization; the Rust adapter has a separate benchmark.
const target={ecs_operation(_layer, encoded) {
  const op=JSON.parse(encoded);
  if (op.op !== 'set' && op.op !== 'set_field') throw new Error('Unexpected operation');
  return '{"found":false,"value":null}';
}};
const scene=await LuaScene.create(new LuaFactory(),target,'gameplay','',[]);
scene.vm.doStringSync(`
  body={enabled=true,collision_enabled=true,gravity_enabled=true,radius=.25,
    capsule_half_height=.675,collision_offset_y=-.775,gravity=18,velocity={0,-.3,0},grounded=true}
  camera={position={1,2,3},view_offset={0,0,0},view_posture_offset_y=0,
    view_bob_pitch=0,view_bob_roll=0,view_turn_roll=0,yaw=.1,pitch=.1,roll=0,
    fov_degrees=71,view_fov_degrees=ecs.null}
`);
for (const [kind, component, field, localName] of [
  ['physics', 'PhysicsBody', 'velocity', 'body'],
  ['camera', 'PlayerCamera', 'position', 'camera'],
]) {
  const loops={
    full:`for i=1,1500 do ecs.set('entity','${component}',${localName}) end`,
    field:`for i=1,1500 do ecs.set_field('entity','${component}','${field}',${localName}.${field}) end`,
  };
  function round(source) {
    scene.vm.global.setTimeout(Date.now()+30000);
    const start=performance.now();
    try {scene.vm.doStringSync(source);} finally {scene.vm.global.setTimeout(undefined);}
    return performance.now()-start;
  }
  for(let i=0;i<3;i++) {round(loops.full);round(loops.field);}
  const samples={full:[],field:[]};
  for(let i=0;i<9;i++)
    for(const variant of (i%2 ? ['field','full'] : ['full','field']))
      samples[variant].push(round(loops[variant]));
  for(const values of Object.values(samples)) values.sort((a,b)=>a-b);
  console.log(JSON.stringify({kind,iterations:1500,full_ms:samples.full[4],field_ms:samples.field[4]}));
}
scene.close();
