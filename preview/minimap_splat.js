// Decode SOG mean positions for maps when a level has no voxel collider.
// The renderer uses the same 16-bit interpolation and inverse log transform.
export function decodeSplatCenters(meta, low, high, flipXY) {
  const count=meta?.count, min=meta?.means?.mins, max=meta?.means?.maxs;
  if (!Number.isSafeInteger(count) || count<1 || count>3_000_000 ||
      !Array.isArray(min) || !Array.isArray(max) || min.length!==3 || max.length!==3 ||
      !min.every(Number.isFinite) || !max.every(Number.isFinite) ||
      !(low instanceof Uint8Array) || !(high instanceof Uint8Array) || low.length<count*4 || high.length<count*4)
    throw new Error('Invalid SOG means for map');
  const points=new Float32Array(count*3);
  const unlog=v=>Math.sign(v)*Math.expm1(Math.abs(v));
  for (let i=0;i<count;i++) for (let axis=0;axis<3;axis++) {
    const q=(low[i*4+axis]+256*high[i*4+axis])/65535;
    const value=unlog(min[axis]+(max[axis]-min[axis])*q);
    points[i*3+axis]=flipXY && axis<2 ? -value : value;
  }
  return points;
}

// North-up, world X to the right and world Z down. The slice represents
// observed splat centers, not collision or guaranteed walkable space.
export function rasterizeSplatSlice(chunks, position, height, radius, thickness, size) {
  const mask=new Uint8Array(size*size), factor=size/(radius*2), half=thickness*.5;
  for (const points of chunks) for (let i=0;i<points.length;i+=3) {
    if (!Number.isFinite(points[i+1]) || Math.abs(points[i+1]-height)>half) continue;
    const x=Math.floor((points[i]-position[0]+radius)*factor);
    const y=Math.floor((points[i+2]-position[2]+radius)*factor);
    if (x<0 || x>=size || y<0 || y>=size) continue;
    // A small footprint keeps sparse scan centers legible without inventing
    // walls: the half-strength edge is at most one map pixel wide.
    for (let oy=-1;oy<=1;oy++) for (let ox=-1;ox<=1;ox++) {
      const px=x+ox, py=y+oy;
      if (px>=0 && px<size && py>=0 && py<size) {
        const index=py*size+px, intensity=(ox===0 && oy===0) ? 255 : 96;
        if (mask[index]<intensity) mask[index]=intensity;
      }
    }
  }
  return mask;
}

// Limit voxel work before dispatch. Include boundary cells conservatively so a
// large map on a fine grid cannot starve its worker with an unbounded query.
export function boundedVoxelRadius(radius, thickness, resolution) {
  if (!Number.isFinite(radius) || !Number.isFinite(thickness) || !Number.isFinite(resolution) || resolution<=0) return radius;
  const yCells=Math.ceil(thickness/resolution)+2;
  const safeRadius=Math.max(.1,(Math.sqrt(14_000_000/yCells)-2)*resolution*.5);
  return Math.min(radius,safeRadius);
}

// A single SOG has no LOD manifest; streamed levels use the finest bounded LOD.
export function selectSplatMapFiles(manifest) {
  if (manifest===null) return ['meta.json'];
  const coarsest=manifest?.lodLevels-1;
  if (!Number.isInteger(coarsest) || coarsest<0 || coarsest>31 || !Array.isArray(manifest.filenames)) throw new Error('invalid map LOD manifest');
  const candidate=Array.isArray(manifest.counts) ? manifest.counts.findIndex(n=>Number.isSafeInteger(n) && n>0 && n<=2_500_000) : -1;
  const lod=candidate>=0 && candidate<=coarsest ? candidate : coarsest;
  const files=manifest.filenames.filter(name=>typeof name==='string' && name.startsWith(`${lod}_`) && /^\d+_\d+\/meta\.json$/.test(name));
  if (!files.length || files.length>32) throw new Error('no bounded map LOD');
  return files;
}
