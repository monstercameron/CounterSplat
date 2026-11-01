import assert from 'node:assert/strict';
import { preloadChunks } from '../preview/chunk_preload.js';

class FakeWorker {
  messages = [];
  postMessage(value) { this.messages.push(value); }
  terminate() { this.terminated = true; }
  send(index, size=2) { this.onmessage({data:{ok:true,index,metadata:new Uint8Array(1),
    dimensions:new Uint32Array(2), pixels:[new Uint8Array(size)]}}); }
  fail(index) { this.onmessage({data:{ok:false,index,error:'bad chunk'}}); }
}
const metas = Array.from({length:5}, () => new Uint8Array([1]));
const filenames = metas.map((_, i) => `${i}/meta.json`);
function setup(maxBytes=1000) {
  const workers = [], controller = new AbortController(), progress = [];
  const promise = preloadChunks({metas, filenames, single:false, base:'http://localhost/', signal:controller.signal,
    size:3, createWorker:() => { const worker = new FakeWorker(); workers.push(worker); return worker; },
    maxBytes, onProgress:(done,total) => progress.push([done,total])});
  return {workers,controller,progress,promise};
}
const run = setup();
assert.deepEqual(run.workers.map(worker => worker.messages[0].index), [0,1,2], 'three chunks start together');
run.workers[1].send(1);
assert.equal(run.workers[1].messages[1].index, 3);
run.workers[0].send(0);
assert.equal(run.workers[0].messages[1].index, 4);
run.workers[2].send(2);
run.workers[0].send(4);
run.workers[1].send(3);
assert.deepEqual((await run.promise).map(chunk => chunk.index), [0,1,2,3,4]);
assert.deepEqual(run.progress.at(-1), [5,5]);
assert(run.workers.every(worker => worker.terminated));
const cancelled = setup();
cancelled.controller.abort();
await assert.rejects(cancelled.promise, error => error.name === 'AbortError');
assert(cancelled.workers.every(worker => worker.terminated));
const failed = setup();
failed.workers[0].fail(0);
await assert.rejects(failed.promise, /Chunk 0: bad chunk/);
assert(failed.workers.every(worker => worker.terminated));
const tooLarge = setup(10);
tooLarge.workers[0].send(0);
await assert.rejects(tooLarge.promise, /exceeds 512 MiB/);
assert(tooLarge.workers.every(worker => worker.terminated));
console.log('Parallel preload ordering, worker limits, cancellation, failure, and memory limit tests passed.');
