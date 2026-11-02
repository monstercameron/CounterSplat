import assert from 'node:assert/strict';
import {SceneAudio} from '../preview/scene_audio.js';
class Node {
  constructor(type) { this.type=type; this.gain={value:1}; this.playbackRate={value:1}; this.connections=[]; }
  connect(node, output=0, input=0) { this.connections.push({node,output,input}); }
  disconnect() { this.disconnected=true; }
  start() { this.started=true; }
  stop() { this.stopped=true; }
}
class Context {
  constructor() { this.state='suspended'; this.destination=new Node('destination'); this.decoded=0; }
  createGain() { return new Node('gain'); }
  createBufferSource() { return new Node('source'); }
  createChannelSplitter() { return new Node('split'); }
  createChannelMerger() { return new Node('merge'); }
  resume() { this.state='running'; return Promise.resolve(); }
  async decodeAudioData(data) { this.decoded++; return {numberOfChannels:new Uint8Array(data)[0]}; }
}
let created=0;
const audio=new SceneAudio(() => {created++; return new Context();});
const scene={resources:{assets:{mono:{kind:'sound',uri:'mono',load:'preload'}, stereo:{kind:'sound',uri:'stereo',load:'preload'}, stream:{kind:'sound',uri:'unused',load:'stream'}}}};
const cache=new Map([['http://test/mono',new Uint8Array([1])],['http://test/stereo',new Uint8Array([2])]]);
await audio.load(scene,'http://test/',cache);
assert.equal(created,0,'no audio device before a user gesture');
audio.setLayer({name:'sound',enabled:true,mix:{gain:.8,muted:false,solo:false}});
audio.unlock(); await audio.decode();
assert.equal(created,1); assert.equal(audio.clips.size,2); assert.equal(audio.context.decoded,2);
const event={asset:'mono',layer:'sound',settings:{gain:.5,balance:0,rate:1,looped:false}};
assert(audio.play(event));
let voice=[...audio.voices][0], [source,split,left,right,merge]=voice.nodes;
assert.equal(source.started,true); assert.equal(split.connections[0].output,0); assert.equal(split.connections[1].output,0);
assert(Math.abs(left.gain.value-.5)<1e-12 && Math.abs(right.gain.value-.5)<1e-12);
assert.equal(merge.connections[0].node,audio.layers.get('sound').node);
source.onended(); assert.equal(audio.voices.size,0); assert(left.disconnected);
assert(audio.play({...event,asset:'stereo',settings:{...event.settings,balance:-1}}));
voice=[...audio.voices][0]; [,split,left,right]=voice.nodes;
assert.equal(split.connections[1].output,1); assert(right.gain.value<1e-12); assert(left.gain.value>.7);
audio.setLayer({name:'music',enabled:true,mix:{gain:.5,muted:false,solo:true}});
assert.equal(audio.layers.get('sound').node.gain.value,0); assert.equal(audio.layers.get('music').node.gain.value,.5);
audio.setLayer({name:'music',enabled:true,mix:{gain:.5,muted:true,solo:true}});
assert.equal(audio.layers.get('music').node.gain.value,0);
assert(!audio.play({...event,asset:'unknown'})); assert(!audio.play({...event,settings:{...event.settings,rate:Infinity}}));
for(let i=0;i<35;i++) assert(audio.play(event));
assert.equal(audio.voices.size,32,'voice count is bounded');
audio.clear(); assert.equal(audio.voices.size,0); assert.equal(audio.clips.size,0);
console.log('Scene audio gesture, decoding, stereo, layer, validation and cleanup tests passed.');
