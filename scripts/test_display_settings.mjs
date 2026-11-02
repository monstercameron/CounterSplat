import assert from 'node:assert/strict';
import {MAX_RENDER_PIXELS, DEFAULT_DISPLAY, ASPECT_RATIOS, validateResolution, normalizeDisplay, aspectLabel,
  aspectMatches, inferAspect, resolutionOptions, presentationMapping, viewportSize, sceneResolution,
  createDisplaySettings, createHudPreferences} from '../preview/display_settings.js';

assert.deepEqual(ASPECT_RATIOS.map(r=>r.id),['16:9','16:10','21:9','custom']);
assert.equal(inferAspect(1920,1080),'16:9');
assert.equal(inferAspect(1920,1200),'16:10');
assert.equal(inferAspect(3440,1440),'21:9');
assert.equal(inferAspect(2560,1080),'21:9');
assert.equal(inferAspect(1024,768),'custom');
assert.equal(validateResolution(1920,1080),null);
assert.match(validateResolution(20,1080),/at least 64/);
assert.match(validateResolution(8192,8192),/megapixel/);
assert.match(validateResolution(4096,2160,2048),/maximum dimension/);
assert.deepEqual(normalizeDisplay({mode:'fixed',width:0,height:1080,scaling:'fill'}),DEFAULT_DISPLAY);
assert.equal(normalizeDisplay({mode:'fixed',width:1024,height:768,scaling:'fit'}).aspect,'custom');
assert.equal(aspectLabel(1920,1080),'16:9');
assert.equal(aspectLabel(1024,768),'4:3');
assert.deepEqual(viewportSize(1920,1080,2),{width:3840,height:2160});
const bounded=viewportSize(12000,3000,2,4096);
assert(bounded.width<=4096 && bounded.height<=4096 && bounded.width*bounded.height<=MAX_RENDER_PIXELS);
assert(Math.abs(bounded.width/bounded.height-4)<0.01);
assert.deepEqual(sceneResolution({mode:'viewport'},{width:1920,height:1080}),{width:1920,height:1080});
const viewport={width:1920,height:1080};
for (const aspect of ['16:9','16:10','21:9']) {
  const options=resolutionOptions(aspect,{width:1920,height:1080},viewport,4096);
  assert(options.length>=4);
  assert(options.every(o=>aspectMatches(aspect,o.width,o.height) && !validateResolution(o.width,o.height,4096)));
  assert(options.some(o=>o.width*o.height<1920*1080));
  assert(options.some(o=>o.width*o.height>1920*1080));
}
const custom=resolutionOptions('custom',{width:1600,height:1200},viewport,4096);
assert(custom.some(o=>o.width===1600 && o.height===1200));
assert(custom.every(o=>Math.abs(o.width/o.height-4/3)<0.002));
assert(!custom.some(o=>o.id==='viewport'), 'a different custom viewport ratio must not enter the list');
assert(!resolutionOptions('custom',{width:1600,height:1200},{width:1024,height:768},4096).some(o=>o.id==='viewport'));
assert.deepEqual(presentationMapping({width:2,height:2},{width:5,height:5},'pixel'),
  {scale:[1.25,1.25],offset:[.125,.125],factor:2,cropped:false});
assert.deepEqual(presentationMapping({width:8,height:6},{width:5,height:5},'pixel'),
  {scale:[.625,5/6],offset:[.5/8,.5/6],factor:1,cropped:true});
assert.deepEqual(presentationMapping({width:2,height:2},{width:5,height:5},'fit').offset,[0,0]);
const data=new Map(); const storage={getItem:key=>data.get(key) ?? null,setItem:(key,value)=>data.set(key,value)};
const settings=createDisplaySettings(storage,()=>4096);
assert.deepEqual(settings.preferences,DEFAULT_DISPLAY);
assert.equal(settings.set({mode:'fixed',width:1920,height:1080,scaling:'pixel',aspect:'16:9'}).ok,true);
assert.match(settings.set({mode:'fixed',width:5000,height:1000,scaling:'fit'}).error,/maximum dimension/);
assert.equal(settings.preferences.width,1920);
assert.equal(settings.setAspect('16:10',viewport).ok,true);
assert.equal(settings.preferences.aspect,'16:10');
assert(aspectMatches('16:10',settings.preferences.width,settings.preferences.height));
assert.equal(settings.setAspect('21:9',viewport).ok,true);
assert(aspectMatches('21:9',settings.preferences.width,settings.preferences.height));
assert.equal(settings.setAspect('custom',viewport).ok,true);
assert.equal(settings.set({mode:'fixed',width:1600,height:1200,scaling:'pixel',aspect:'custom'}).ok,true);
assert.equal(settings.preferences.aspect,'custom');
// A native 1280×605 ratio must not drift when stepping through rounded sizes.
assert.equal(settings.set({mode:'fixed',width:1280,height:605,scaling:'pixel',aspect:'custom'}).ok,true);
for (const height of [600,270,720,600]) {
  const ratio={width:settings.preferences.ratio_width,height:settings.preferences.ratio_height};
  const choices=resolutionOptions('custom',settings.preferences,viewport,4096,ratio);
  const width=Math.round(height*1280/605);
  assert(choices.some(choice=>choice.width===width && choice.height===height));
  assert.equal(settings.set({mode:'fixed',width,height,scaling:'pixel',aspect:'custom',
    ratio_width:ratio.width,ratio_height:ratio.height}).ok,true);
  assert.deepEqual([settings.preferences.ratio_width,settings.preferences.ratio_height],[1280,605]);
}
assert.equal(settings.set({mode:'fixed',width:settings.preferences.width,height:settings.preferences.height,
  scaling:'fit',aspect:'custom'}).ok,true);
assert.deepEqual([settings.preferences.ratio_width,settings.preferences.ratio_height],[1280,605]);
assert.equal(settings.set({mode:'fixed',width:settings.preferences.width,height:settings.preferences.height,
  scaling:'fit',aspect:'custom',reset_ratio:true}).ok,true);
assert.deepEqual([settings.preferences.ratio_width,settings.preferences.ratio_height],[1269,600]);
assert.equal(settings.set({mode:'fixed',width:1920,height:1080,scaling:'fit',aspect:'bad'}).ok,false);
assert.equal(settings.setAspect('16:9',viewport).ok,true);
assert.equal('ratio_width' in settings.preferences,false);
assert.deepEqual(createDisplaySettings(storage,()=>4096).preferences,settings.preferences);
const hud=createHudPreferences(storage); assert.equal(hud.showPerformance,true);
assert.equal(hud.set(false),true); assert.equal(createHudPreferences(storage).showPerformance,false);
assert.equal(hud.set('false'),false); assert.equal(hud.showPerformance,false);
console.log('Aspect ratios, safe resolution choices, pixel presentation, and persistence passed.');
