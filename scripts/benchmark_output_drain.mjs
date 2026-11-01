import {performance} from 'node:perf_hooks';
import {LuaScene} from '../preview/lua_runtime.js';
const names=['player.controls_changed','camera.lens_changed','camera.zoom_lens_changed',
 'camera.controller_changed','image.correction_changed','first_person.frame'];
const frame={clip:'lowReady',index:5,motion_x:.002,motion_y:.003,motion_roll:.0002};
class Target {
  constructor(){this.events=new Map();}
  ecs_operation(_layer,raw){
    const op=JSON.parse(raw);
    if(op.op==='drain') {const value=this.events.get(op.name)||[];this.events.delete(op.name);return JSON.stringify({found:true,value});}
    if(op.op==='drain_next') {
      for(let i=op.start||0;i<op.names.length;i++) {
        const value=this.events.get(op.names[i]);
        if(value?.length){this.events.delete(op.names[i]);return JSON.stringify({found:true,value:{index:i,events:value}});}
      }
      return JSON.stringify({found:false,value:null});
    }
    throw new Error('unexpected');
  }
}
const target=new Target(),scene=new LuaScene(null,target,'gameplay');
function full(){
  for(const name of names){const events=scene.drain(name); for(const event of events) if(!event.clip) throw new Error('bad event');}
}
function next(){
  scene.consumePending(names, (_index, events) => {
    for(const event of events)if(!event.clip)throw new Error('bad event');
  });
}
function run(fn){
  const t=performance.now();
  for(let i=0;i<20000;i++){if(i%2===0)target.events.set('first_person.frame',[frame]);fn();}
  return performance.now()-t;
}
for(let i=0;i<3;i++){run(full);run(next);}
const a=[],b=[];for(let i=0;i<11;i++){if(i%2){b.push(run(next));a.push(run(full))}else{a.push(run(full));b.push(run(next))}}
a.sort((x,y)=>x-y);b.sort((x,y)=>x-y);console.log(JSON.stringify({frames:20000,with_frame_fraction:.5,full_ms:a[5],next_ms:b[5]}));
