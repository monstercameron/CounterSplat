import assert from 'node:assert/strict';
import {MAX_RENDER_PIXELS, DEFAULT_DISPLAY, validateResolution, normalizeDisplay, aspectLabel, viewportSize, sceneResolution, attachDisplayPanel} from '../preview/display_settings.js';

assert.equal(validateResolution(1920,1080),null);
assert.equal(validateResolution(3440,1440),null);
assert.match(validateResolution(20,1080),/at least 64/);
assert.match(validateResolution(8192,8192),/megapixel/);
assert.match(validateResolution(4096,2160,2048),/maximum dimension/);
assert.deepEqual(normalizeDisplay({mode:'fixed',width:0,height:1080,scaling:'fill'}),DEFAULT_DISPLAY);
assert.equal(aspectLabel(1920,1080),'16:9');
assert.equal(aspectLabel(1024,768),'4:3');
assert.deepEqual(viewportSize(1920,1080,1),{width:1920,height:1080});
assert.deepEqual(viewportSize(1920,1080,2),{width:3840,height:2160});
const bounded=viewportSize(12000,3000,2,4096);
assert(bounded.width<=4096 && bounded.height<=4096 && bounded.width*bounded.height<=MAX_RENDER_PIXELS);
assert(Math.abs(bounded.width/bounded.height-4)<0.01);
assert.deepEqual(sceneResolution({mode:'fixed',width:1024,height:768},{width:1920,height:1080}),{width:1024,height:768});
assert.deepEqual(sceneResolution({mode:'viewport'},{width:1920,height:1080}),{width:1920,height:1080});

class Element {
  constructor(){this.listeners={};this.value='';this.hidden=false;this.textContent='';this.children=[];}
  addEventListener(name,fn){this.listeners[name]=fn;}
  append(child){this.children.push(child);}
  focus(){this.focused=true;}
  trigger(name){this.listeners[name]?.({preventDefault(){}});}
}
const ids=['render-preset','viewport-scaling','custom-resolution','render-width','render-height','resolution-form','display-feedback','display-current'];
const elements=Object.fromEntries(ids.map(id=>[id,new Element()]));
const document={getElementById:id=>elements[id],createElement:()=>new Element()};
const data=new Map(); const storage={getItem:key=>data.get(key) ?? null,setItem:(key,value)=>data.set(key,value)};
const panel=attachDisplayPanel(document,storage,()=>4096);
assert.deepEqual(panel.preferences,DEFAULT_DISPLAY);
elements['render-preset'].value='1920x1080';elements['render-preset'].trigger('change');
assert.deepEqual(sceneResolution(panel.preferences,{}),{width:1920,height:1080});
elements['render-preset'].value='custom';elements['render-preset'].trigger('change');
assert.equal(elements['custom-resolution'].hidden,false);
elements['render-width'].value='5000';elements['render-height'].value='1000';elements['resolution-form'].trigger('submit');
assert.equal(panel.preferences.width,1920);assert.match(elements['display-feedback'].textContent,/maximum dimension/);
elements['render-width'].value='1600';elements['render-height'].value='1200';elements['resolution-form'].trigger('submit');
assert.equal(panel.preferences.width,1600);assert.equal(panel.preferences.height,1200);
elements['viewport-scaling'].value='fit';elements['viewport-scaling'].trigger('change');
assert.equal(JSON.parse(data.get('countersplat.display.v1')).scaling,'fit');
elements['render-preset'].value='viewport';elements['render-preset'].trigger('change');
panel.updateViewport({width:1440,height:900});assert.match(elements['display-feedback'].textContent,/1440 × 900/);
console.log('Resolution validation, proportional viewport sizing, persistence, and display controls passed.');
