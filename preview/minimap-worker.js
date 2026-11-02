// Map rasterization owns a separate copy of level voxels so a map request never
// blocks the collision worker or the render thread.
import init, { VoxelCollisionData, decode_texture } from '/pkg/countersplat_web.js';
import {decodeSplatCenters, rasterizeSplatSlice, boundedVoxelRadius, selectSplatMapFiles} from './minimap_splat.js';
const wasmReady = init();
let collider = null, splats = null;
const readBytes = async (url, limit) => {
  const response=await fetch(url,{cache:'no-store'});
  if (!response.ok) throw new Error(`map asset HTTP ${response.status}`);
  if (Number(response.headers.get('content-length') || 0)>limit) throw new Error('map asset exceeds size limit');
  const bytes=new Uint8Array(await response.arrayBuffer());
  if (bytes.length>limit) throw new Error('map asset exceeds size limit');
  return bytes;
};
const readJson = async (url, limit) => JSON.parse(new TextDecoder().decode(await readBytes(url,limit)));
async function loadSplatFallback(base, flipXY) {
  // A missing LOD manifest denotes a single-SOG level. Streamed levels use
  // the finest complete LOD within the map worker's memory budget.
  const manifestUrl=new URL('lod-meta.json',base);
  const response=await fetch(manifestUrl,{cache:'no-store'});
  let manifest=null;
  if (response.status!==404) {
    if (!response.ok || Number(response.headers.get('content-length') || 0)>16*1024*1024) throw new Error('invalid map LOD response');
    const bytes=new Uint8Array(await response.arrayBuffer());
    if (bytes.length>16*1024*1024) throw new Error('map LOD manifest exceeds size limit');
    manifest=JSON.parse(new TextDecoder().decode(bytes));
  }
  const files=selectSplatMapFiles(manifest);
  const chunks=[]; let total=0;
  for (const file of files) {
    const url=new URL(file,base), meta=await readJson(url,1024*1024);
    if (!Number.isSafeInteger(meta.count) || meta.count<1 || (total+=meta.count)>3_000_000) throw new Error('map splat count exceeds budget');
    const [loBytes,hiBytes]=await Promise.all([
      readBytes(new URL('means_l.webp',url),32*1024*1024),readBytes(new URL('means_u.webp',url),32*1024*1024)]);
    const low=decode_texture(0,loBytes), high=decode_texture(1,hiBytes);
    try {
      if (low.width()!==high.width() || low.height()!==high.height()) throw new Error('map means dimensions differ');
      chunks.push(decodeSplatCenters(meta,low.take_pixels(),high.take_pixels(),flipXY));
    } finally { low.free(); high.free(); }
  }
  return chunks;
}
self.onmessage = async ({data}) => {
  try {
    if (data.type === 'load') {
      const metadata = await fetch(new URL('scene.voxel.json', data.base), {cache:'no-store'});
      if (metadata.status === 404) {
        await wasmReady;
        try { splats=await loadSplatFallback(data.base,data.flipXY!==false); }
        catch (error) { console.warn('Splat map unavailable:',error); self.postMessage({type:'unavailable'}); return; }
        self.postMessage({type:'ready'}); return;
      }
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
      self.postMessage({type:'ready'});
    } else if (data.type === 'slice' && (collider || splats)) {
      const {id, position, feet, units, radius, height, thickness, size} = data;
      // Use a real floor when available; retain the calibrated foot height when
      // a floor probe cannot find enough support (for example while flying).
      const probe = new Float32Array([position[0], Math.max(position[1], feet + .1 * units), position[2]]);
      const ground = collider?.ground_height(probe, Math.min(.15 * units, 1), Math.min(6 * units, 16));
      const floor = Number.isFinite(ground) ? ground : feet;
      const sliceHeight = floor + height;
      const mapRadius = collider ? boundedVoxelRadius(radius, thickness, collider.resolution()) : radius;
      const mask = collider ? collider.horizontal_slice(new Float32Array([position[0], position[2]]), sliceHeight, mapRadius, thickness, size) :
        rasterizeSplatSlice(splats, position, sliceHeight, mapRadius, thickness, size);
      if (mask.length !== size * size) throw new Error('invalid map raster size');
      const pixels = new Uint8Array(size * size * 4);
      for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
        const i = y * size + x, p = i * 4;
        const dx = (x + .5) / size * 2 - 1, dy = (y + .5) / size * 2 - 1;
        if (dx * dx + dy * dy > .97 * .97) continue;
        if (mask[i]) { pixels[p]=190; pixels[p+1]=211; pixels[p+2]=208; pixels[p+3]=Math.round(135+90*mask[i]/255); }
        else { pixels[p]=10; pixels[p+1]=20; pixels[p+2]=25; pixels[p+3]=178; }
      }
      self.postMessage({type:'slice', id, center:[position[0],position[2]], radius:mapRadius, height:sliceHeight, size, pixels}, [pixels.buffer]);
    }
  } catch (error) {
    self.postMessage({type:'error', operation:data?.type, id:data?.id, message:String(error).slice(0,256)});
  }
};
