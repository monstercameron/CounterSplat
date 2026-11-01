import assert from 'node:assert/strict';
import { metadataUrl, prepareMetadata } from '../preview/metadata_loader.js';
import { loadMetadataInWorker } from '../preview/metadata_client.js';

const base = 'http://localhost/assets/levels/1234abcd/';
assert.equal(metadataUrl('0_0/meta.json', base), `${base}0_0/meta.json`);
for (const path of ['/secret', '../secret', 'x/../secret', 'x\\secret', 'x//secret']) {
  assert.throws(() => metadataUrl(path, base));
}
assert.equal(metadataUrl('%2e%2e/secret', base), `${base}%252e%252e/secret`);

const encoder = new TextEncoder();
const level = {id:'1234abcd', name:'Example', format:'streamed_sog'};
const filenames = Array.from({length:7}, (_, index) => `${index}/meta.json`);
let active = 0, peak = 0;
const progress = [];
const request = {level, lodCount:2, filenames, base, concurrency:3,
  inventoryBytes:encoder.encode(JSON.stringify({scene_id:level.id,files:[{path:'scene.voxel.bin'}]})),
  viewerSettings:true, hasSource:true};
const result = await prepareMetadata(request, {onProgress:(done,total) => progress.push([done,total]), fetcher:async url => {
  active++; peak = Math.max(peak, active);
  const index = Number(new URL(url).pathname.split('/').at(-2));
  await new Promise(resolve => setTimeout(resolve, (7 - index) * 2));
  active--;
  const bytes = encoder.encode(JSON.stringify({means:{files:[`stream-${index}.webp`]}}));
  return {ok:true, headers:{get:() => null}, arrayBuffer:async () => bytes.buffer};
}});
assert.equal(peak, 3, 'fetches overlap up to the configured bound');
assert.equal(progress.length, filenames.length);
assert.deepEqual(progress.at(-1), [7,7]);
assert.deepEqual(result.metas.map(bytes => JSON.parse(new TextDecoder().decode(bytes)).means.files[0]),
  filenames.map((_, index) => `stream-${index}.webp`), 'completion order does not change file order');
assert(result.levelAssets.files.some(file => file.path === 'scene.voxel.bin' && file.kind === 'collision'));
assert(result.levelAssets.files.some(file => file.path === '2/stream-2.webp'));
await assert.rejects(prepareMetadata({...request, concurrency:0}), /Invalid level metadata/);
await assert.rejects(prepareMetadata({...request, filenames:['bad/meta.json']}, {fetcher:async () => ({ok:false,status:404})}), /HTTP 404/);
await assert.rejects(prepareMetadata({...request, filenames:['bad/meta.json']}, {fetcher:async () => ({ok:true,headers:{get:() => String(17*1024*1024)}})}), /16 MiB/);
const single = await prepareMetadata({level:{...level,format:'single_sog'}, lodCount:1, filenames:['single/meta.json'], base,
  singleMetadata:encoder.encode(JSON.stringify({means:{files:['means_l.webp']}}))}, {fetcher:() => { throw Error('unexpected fetch'); }});
assert(single.levelAssets.files.some(file => file.path === 'means_l.webp'));

class FakeWorker {
  listeners = new Map();
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  removeEventListener(name) { this.listeners.delete(name); }
  postMessage(value, transfer) { this.request = value; this.transfer = transfer; }
  terminate() { this.terminated = true; }
  send(data) { this.listeners.get('message')?.({data}); }
  fail(message) { this.listeners.get('error')?.({message}); }
}
const worker = new FakeWorker(), controller = new AbortController(), reports = [];
const wait = loadMetadataInWorker(worker, request, controller.signal, (done,total) => reports.push([done,total]));
assert.deepEqual(worker.transfer, [request.inventoryBytes.buffer]);
worker.send({type:'progress',completed:1,total:7});
worker.send({type:'done',result});
assert.equal(await wait, result);
assert.deepEqual(reports, [[1,7]]);
assert(worker.terminated);
assert.equal(worker.listeners.size, 0);
const cancelledWorker = new FakeWorker(), cancelled = new AbortController();
const cancelledPromise = loadMetadataInWorker(cancelledWorker, request, cancelled.signal);
cancelled.abort();
await assert.rejects(cancelledPromise, error => error.name === 'AbortError');
assert(cancelledWorker.terminated);
const brokenWorker = new FakeWorker();
const brokenPromise = loadMetadataInWorker(brokenWorker, request, new AbortController().signal);
brokenWorker.fail('worker failure');
await assert.rejects(brokenPromise, /worker failure/);
assert(brokenWorker.terminated);
const badWorker = new FakeWorker();
const badPromise = loadMetadataInWorker(badWorker, request, new AbortController().signal);
badWorker.send({type:'progress',completed:8,total:7});
await assert.rejects(badPromise, /invalid progress/);
assert(badWorker.terminated);
console.log('Metadata preparation, bounded concurrency, transfer, error, and cancellation tests passed.');
