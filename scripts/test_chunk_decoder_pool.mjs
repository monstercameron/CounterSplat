import assert from 'node:assert/strict';
import {ChunkDecoderPool, chunkDecoderCount} from '../preview/chunk_decoder_pool.js';

class FakeWorker {
  messages = [];
  postMessage(value) { this.messages.push(value); }
  terminate() { this.terminated = true; }
  send(value) { this.onmessage({data:value}); }
  crash(message='worker crashed') { this.onerror({message}); }
}
const workers = [], results = [], failures = [], fatal = [];
const pool = new ChunkDecoderPool({
  size: 3,
  createWorker: () => { const worker = new FakeWorker(); workers.push(worker); return worker; },
  onResult: data => results.push(data.index),
  onFailure: (index, error) => failures.push([index, String(error)]),
  onFatal: error => fatal.push(error),
});
assert.deepEqual([1,2,4,8,16,undefined].map(cores => chunkDecoderCount(cores)), [1,1,2,3,4,2]);
assert.equal(chunkDecoderCount(16, 1), 1);
assert.equal(chunkDecoderCount(1, 4), 4);
assert.throws(() => chunkDecoderCount(8, 0), /workers must/);
assert.throws(() => chunkDecoderCount(8, 5), /workers must/);
assert.throws(() => chunkDecoderCount(8, 'many'), /workers must/);
assert.equal(pool.available, 3);
assert(pool.submit({index:1}));
assert(pool.submit({index:2}));
assert(pool.submit({index:3}));
assert.equal(pool.available, 0);
assert.equal(pool.submit({index:4}), false, 'no hidden queue');
workers[1].send({ok:true,index:2});
assert.deepEqual(results, [2]);
assert.equal(pool.available, 1);
assert(pool.submit({index:4}));
workers[1].send({ok:false,index:4,error:'bad data'});
assert.deepEqual(failures, [[4,'bad data']]);
workers[0].crash();
assert.equal(workers[0].terminated, true);
assert.deepEqual(failures.at(-1), [1,'worker crashed']);
assert.equal(pool.size, 3, 'failed active worker is replaced');
workers[0].send({ok:true,index:1});
assert.deepEqual(results, [2], 'late results from a failed worker are ignored');
assert(pool.submit({index:5}));
workers.at(-1).send({ok:true,index:5});
assert.deepEqual(results, [2,5]);
const surviving = workers.filter(worker => !worker.terminated);
pool.stop();
assert(surviving.every(worker => worker.terminated));
workers[2].send({ok:true,index:3});
assert.deepEqual(results, [2,5], 'results from a previous scene are ignored');
assert.equal(pool.available, 0);
assert.deepEqual(fatal, []);
console.log('Bounded parallel chunk decoder tests passed.');
