import assert from 'node:assert/strict';
import { GameplayFireButton, GameplayMouseButtons } from '../preview/mouse_buttons.js';
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
// A real mouse chord reports the second button as pointermove, not pointerdown.
const chord=[];
const aim=new GameplayMouseButtons(value=>chord.push(['aim',value]));
const fire=new GameplayFireButton(value=>chord.push(['fire',value]));
for (const event of [pointer('pointerdown',2,2),pointer('pointermove',0,3),
                     pointer('pointermove',0,2),pointer('pointerup',2,0)]) {
  aim.update(event,true); fire.update(event,true);
}
assert.deepEqual(chord,[['aim',true],['fire',true],['fire',false],['aim',false]]);

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
assert.equal(player.presentation().visible,false,'undecoded footage has no presented anchor');
const resolve=(name)=>pending.get('/scene/'+name+'.webp')({ok:true,blob:async()=>({id:name})});
resolve('a'); resolve('c'); resolve('b');
for(let i=0;i<4;i++) await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(copied,['c'],'late frames must never replace the currently requested frame');
assert.equal(player.presentation().index,3,'presentation reports the installed frame');
assert.equal(settings.length,2);
player.request(frame(1)); assert.deepEqual(copied,['c','a'],'cached frames upload immediately');
player.clips.idle={frames:['c']};
player.request({...frame(1),clip:'idle',transition_seconds:.1,motion_x:.01,motion_y:-.02,motion_roll:.03});
assert.equal(captures.length,1,'a discontinuous clip change captures the previous frame');
assert.equal(motions.at(-1)[3],0,'the new clip begins at the old frame');
assert.deepEqual(player.presentation().previous,{clip:'raising',index:1});
assert.equal(player.presentation().clip,'idle');
assert.equal(player.presentation().blend,0);
player.update(1000); player.update(1050); assert(Math.abs(motions.at(-1)[3]-.5)<1e-8);
assert(Math.abs(player.presentation().blend-.5)<1e-8);
player.update(1100); assert.equal(motions.at(-1)[3],1,'the clip handoff completes');
assert.equal(player.presentation().previous,null);
assert.deepEqual(motions.at(-1).slice(0,3),[.01,-.02,.03]);
player.clips.loop={frames:['a','b','c']};
player.request({...frame(3),clip:'loop',loop:true,direction:1});
const prior=captures.length;
player.request({...frame(1),clip:'loop',loop:true,direction:1,loop_blend_seconds:.1});
assert.equal(captures.length,prior+1,'a looping idle blends its last frame into its first');
// Several fixed ticks may finish before a single canvas submission. The
// authored flash must survive later smoke/idle frames until that submission.
player.clips.fireADS={frames:['a','b','c']};
const flash={...frame(1),clip:'fireADS'};
const idle={...frame(1),clip:'idle'};
assert.equal(player.requestBatch([flash,{...flash,index:3},idle]),flash);
assert.equal(player.uploaded,'a');
player.requestBatch([{...idle,motion_x:.02}]);
assert.equal(player.uploaded,'a','later animation must not overwrite an unpresented flash');
assert.equal(player.presented().motion_x,.02);
assert.equal(player.uploaded,'c','the newest state follows the rendered flash');
player.close(); assert.deepEqual(closed.sort(),['a','b','c']);
pending.clear();
const flashAssets={...assets,hip:{kind:'first_person_frame',uri:'hip.webp'},ads:{kind:'first_person_frame',uri:'ads.webp'}};
const warm=new FirstPersonFrames({resources:{assets:flashAssets}},'http://local.test/scene/',
  {raising:{frames:['a','b','c']},fireHip:{frames:['hip']},fireADS:{frames:['ads']}},new Map(),{
    set_first_person_settings(){}, set_first_person_motion(){}, install_first_person_frame(){},
    capture_first_person_frame(){return false}
  });
assert.deepEqual([...pending.keys()].slice(0,2),['/scene/hip.webp','/scene/ads.webp'],
  'brief firing frames must load before the longer raise animation');
assert(warm.pinned.has('hip') && warm.pinned.has('ads'),'muzzle flashes remain resident');
const delayedFlash={...frame(1),clip:'fireADS'}, delayedIdle={...frame(1),clip:'raising'};
warm.requestBatch([delayedFlash,delayedIdle]);
assert.equal(warm.presentation().visible,false,'a pending image cannot be reported as displayed');
assert.equal(warm.presented(),null,'a not-yet-decoded flash must not be skipped');
assert.equal(warm.desired,'ads');
resolve('ads');
for(let i=0;i<4;i++) await new Promise(resolve=>setImmediate(resolve));
assert.equal(warm.uploaded,'ads');
assert.equal(warm.presented(),delayedIdle,'the newest state follows once the flash is drawn');
assert.equal(warm.presentation().clip,'fireADS','a requested but undecoded clip is not yet displayed');
assert.equal(warm.presentation().index,1);
warm.close();
console.log('First-person mouse gating, stale-frame protection, caching, and disposal tests passed.');
