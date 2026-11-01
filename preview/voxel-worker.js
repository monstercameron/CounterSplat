// The voxel worker owns sparse collision data and CPU queries, separate from the WebGPU owner.
import init, { VoxelCollisionData } from '/pkg/countersplat_web.js';
const wasmReady = init();
let collider = null;
self.onmessage = async ({data}) => {
  try {
    if (data.type === 'load') {
      const metadata = await fetch(new URL('scene.voxel.json', data.base), {cache:'no-store'});
      if (metadata.status === 404) { self.postMessage({type:'unavailable'}); return; }
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
      self.postMessage({type:'ready', resolution:collider.resolution()});
    } else if (data.type === 'move' && collider) {
      let position = new Float32Array(data.position);
      for (const delta of data.deltas) position = collider.move_capsule(position, new Float32Array(delta), data.radius, data.halfHeight ?? 0);
      self.postMessage({type:'move', id:data.id, position:Array.from(position)});
    } else if (data.type === 'ground' && collider) {
      const height = collider.ground_height(new Float32Array(data.position), data.radius, data.maxDrop);
      self.postMessage({type:'ground', id:data.id, height});
    } else if (data.type === 'debug' && collider) {
      const points = collider.nearby(new Float32Array(data.position), data.radius, data.limit);
      self.postMessage({type:'debug', id:data.id, points}, [points.buffer]);
    }
  } catch (error) {
    self.postMessage({type:'error', operation:data.type, id:data.id, message:String(error)});
  }
};
