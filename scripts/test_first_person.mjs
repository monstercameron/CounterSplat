import assert from 'node:assert/strict';
import { GameplayFireButton } from '../preview/mouse_buttons.js';
import { FirstPersonFrames } from '../preview/first_person_frames.js';

const events=[];
const button=new GameplayFireButton(value=>events.push(value));
const pointer=(type,button,buttons)=>({pointerType:'mouse',type,button,buttons});
// A press on UI cannot begin firing when dragged into the scene.
button.update(pointer('pointerdown',0,1),false);
button.update(pointer('pointermove',-1,1),true);
button.update(pointer('pointerup',0,0),false);
assert.deepEqual(events,[]);
// Mouse chords and focus loss must not leave the trigger held.
button.update(pointer('pointerdown',2,2),true);
button.update(pointer('pointermove',0,3),true);
button.reset(); button.update(pointer('pointermove',-1,3),true);
button.update(pointer('pointerup',0,2),false);
assert.deepEqual(events,[true,false]);

const pending=new Map(), copied=[], settings=[], closed=[], motions=[], captures=[];
globalThis.location={origin:'http://local.test'};
globalThis.fetch=(url) => new Promise(resolve => pending.set(url.pathname, resolve));
globalThis.createImageBitmap=async blob => ({id:blob.id,close(){closed.push(blob.id)}});
const assets={a:{kind:'first_person_frame',uri:'a.webp'},b:{kind:'first_person_frame',uri:'b.webp'},c:{kind:'first_person_frame',uri:'c.webp'}};
const player=new FirstPersonFrames({resources:{assets}},'http://local.test/scene/',{raising:{frames:['a','b','c']}},new Map(),{
  install_first_person_frame(bitmap){copied.push(bitmap.id)},
  set_first_person_settings(...args){settings.push(args)},
  set_first_person_motion(...args){motions.push(args)},
  capture_first_person_frame(){captures.push(true);return true}
});
const frame=index=>({clip:'raising',index,visible:true,opacity:1,key_threshold:.14,key_softness:.28,despill:.025,vertical_offset:.055});
player.request(frame(1)); player.request(frame(3));
const resolve=(name)=>pending.get('/scene/'+name+'.webp')({ok:true,blob:async()=>({id:name})});
resolve('a'); resolve('c'); resolve('b');
for(let i=0;i<4;i++) await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(copied,['c'],'late frames must never replace the currently requested frame');
assert.equal(settings.length,2);
player.request(frame(1)); assert.deepEqual(copied,['c','a'],'cached frames upload immediately');
player.clips.idle={frames:['c']};
player.request({...frame(1),clip:'idle',transition_seconds:.1,motion_x:.01,motion_y:-.02,motion_roll:.03});
assert.equal(captures.length,1,'a discontinuous clip change captures the previous frame');
assert.equal(motions.at(-1)[3],0,'the new clip begins at the old frame');
player.update(1000); player.update(1050); assert(Math.abs(motions.at(-1)[3]-.5)<1e-8);
player.update(1100); assert.equal(motions.at(-1)[3],1,'the clip handoff completes');
assert.deepEqual(motions.at(-1).slice(0,3),[.01,-.02,.03]);
player.clips.loop={frames:['a','b','c']};
player.request({...frame(3),clip:'loop',loop:true,direction:1});
const prior=captures.length;
player.request({...frame(1),clip:'loop',loop:true,direction:1,loop_blend_seconds:.1});
assert.equal(captures.length,prior+1,'a looping idle blends its last frame into its first');
player.close(); assert.deepEqual(closed.sort(),['a','b','c']);
console.log('First-person mouse gating, stale-frame protection, caching, and disposal tests passed.');
