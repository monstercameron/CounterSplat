// The voxel worker owns sparse collision data and CPU queries, separate from the WebGPU owner.
import init, { VoxelCollisionData } from '/pkg/countersplat_web.js';
const wasmReady = init();
let collider = null;
// Timing is requested only by the local profiler. The measured worker span
// includes query execution and result packing, not time queued on either side.
function respond(payload, started, transfer) {
  if (started !== null) payload.worker_ms = Math.max(0, performance.now() - started);
  if (transfer) self.postMessage(payload, transfer); else self.postMessage(payload);
}
self.onmessage = async ({data}) => {
  const started = data?.profile === true ? performance.now() : null;
  try {
    if (data.type === 'load') {
      const metadata = await fetch(new URL('scene.voxel.json', data.base), {cache:'no-store'});
      if (metadata.status === 404) { respond({type:'unavailable'}, started); return; }
      if (!metadata.ok) throw new Error(`voxel metadata HTTP ${metadata.status}`);
      const info = new Uint8Array(await metadata.arrayBuffer());
      if (info.byteLength > 1024 * 1024) throw new Error('voxel metadata exceeds 1 MiB');
      const response = await fetch(new URL('scene.voxel.bin', data.base), {cache:'no-store'});
      if (!response.ok) throw new Error(`voxel binary HTTP ${response.status}`);
      if (Number(response.headers.get('content-length') || 0) > 128 * 1024 * 1024) throw new Error('voxel binary exceeds 128 MiB');
      const bytes = new Uint8Array(await response.arrayBuffer());
      if (bytes.byteLength > 128 * 1024 * 1024) throw new Error('voxel binary exceeds 128 MiB');
      await wasmReady;
      collider = new VoxelCollisionData(info, bytes);
      respond({type:'ready', resolution:collider.resolution()}, started);
    } else if (data.type === 'move' && collider) {
      let position = new Float32Array(data.position);
      for (const delta of data.deltas) position = collider.move_capsule(position, new Float32Array(delta), data.radius, data.halfHeight ?? 0);
      respond({type:'move', id:data.id, position:Array.from(position)}, started);
    } else if (data.type === 'trace' && collider) {
      const hit = collider.trace_segment(new Float32Array(data.start), new Float32Array(data.end));
      respond({type:'trace', id:data.id, hit:Array.from(hit)}, started);
    } else if (data.type === 'clearance' && collider) {
      const clear = collider.capsule_clear(new Float32Array(data.center), data.radius, data.halfHeight);
      respond({type:'clearance', id:data.id, clear}, started);
    } else if (data.type === 'ground' && collider) {
      const height = collider.ground_height(new Float32Array(data.position), data.radius, data.maxDrop);
      respond({type:'ground', id:data.id, height}, started);
    } else if (data.type === 'debug' && collider) {
      const points = collider.nearby(new Float32Array(data.position), data.radius, data.limit);
      respond({type:'debug', id:data.id, points}, started, [points.buffer]);
    }
  } catch (error) {
    respond({type:'error', operation:data.type, id:data.id, message:String(error)}, started);
  }
};
