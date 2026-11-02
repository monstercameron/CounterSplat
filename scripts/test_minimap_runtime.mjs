import assert from 'node:assert/strict';
import {MinimapRuntime} from '../preview/minimap_runtime.js';
class Worker {
  messages=[];
  postMessage(value) { this.messages.push(value); }
  terminate() { this.terminated=true; }
  receive(value) { this.onmessage({data:value}); }
}
const worker=new Worker(), uploads=[], results=[];
let current=true;
const game={set_gameplay_ui_image:(...args)=>uploads.push(args)};
const map=new MinimapRuntime(game,new URL('http://localhost/level/'),()=>current,result=>results.push(result),worker);
const request=(x)=>({position:[x,2,0],feet:.3,units:1,radius:12,height:.9144,thickness:.4,size:32});
map.request(request(0)); map.request(request(1));
assert.equal(worker.messages.length,1,'requests wait until the worker is ready');
worker.receive({type:'ready'});
assert.equal(worker.messages[1].position[0],1,'loading requests coalesce');
map.request(request(2)); map.request(request(3));
assert.equal(worker.messages.length,2,'only one request is in flight');
worker.receive({type:'slice',id:999,size:32,pixels:new Uint8Array(4096),center:[0,0],radius:12,height:1});
assert.equal(uploads.length,0,'stale replies are ignored');
worker.receive({type:'slice',id:worker.messages[1].id,size:32,pixels:new Uint8Array(4096),center:[1,0],radius:12,height:1});
assert.equal(uploads.length,1); assert.deepEqual(results[0].center,[1,0]);
assert.equal(worker.messages[2].position[0],3,'only the newest waiting request is sent');
worker.receive({type:'slice',id:worker.messages[2].id,size:32,pixels:new Uint8Array(5),center:[3,0],radius:12,height:1});
assert.equal(results.at(-1).status,'error','invalid rasters cannot be uploaded');
map.request({...request(0),radius:Infinity}); assert.equal(worker.messages.length,3);
current=false; worker.receive({type:'unavailable'}); assert.equal(results.length,2,'an old scene cannot change the current HUD');
map.stop(); assert(worker.terminated);
console.log('Minimap worker coalescing, bounds, stale results, and cleanup passed.');
