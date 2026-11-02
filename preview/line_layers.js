// Independent ECS scripts may publish world-line layers without clearing one
// another. The host validates and packs presentation data, not gameplay rules.
export const MAX_LINE_SEGMENTS = 1024;
const DEFAULT_COLOR = [3.2, 1.55, 0.35];
const BOX_EDGES = [[0,1],[0,2],[0,4],[1,3],[1,5],[2,3],[2,6],[3,7],[4,5],[4,6],[5,7],[6,7]];
const finiteVector = (v,n) => Array.isArray(v) && v.length === n && v.every(Number.isFinite);
export class LineLayers {
  constructor() { this.layers = new Map(); }
  clear() { this.layers.clear(); }
  update(snapshot) {
    if (!snapshot || typeof snapshot !== 'object') return false;
    const layer = snapshot.layer ?? 'default';
    if (typeof layer !== 'string' || !/^[a-zA-Z0-9_.-]{1,64}$/.test(layer)) return false;
    if (!this.layers.has(layer) && this.layers.size >= 8) return false;
    const segments = [];
    for (const line of (Array.isArray(snapshot.segments) ? snapshot.segments : []).slice(0, MAX_LINE_SEGMENTS)) {
      if (!line || !Array.isArray(line.start) || line.start.length !== 3 ||
          !Array.isArray(line.finish) || line.finish.length !== 3) continue;
      const color = line.color === undefined ? DEFAULT_COLOR : line.color;
      if (!Array.isArray(color) || color.length !== 3 || !color.every(v => Number.isFinite(v) && v >= 0 && v <= 8) ||
          ![...line.start, ...line.finish, line.alpha].every(Number.isFinite) ||
          [...line.start, ...line.finish].some(v => Math.abs(v) > 10_000_000) || line.alpha < 0 || line.alpha > 1) continue;
      segments.push([...line.start, ...line.finish, ...color, line.alpha * 0.9]);
    }
    // Expand compact oriented boxes into the same line representation. Lua
    // supplies the geometry and colors; this is presentation-only packing.
    for (const box of (Array.isArray(snapshot.boxes) ? snapshot.boxes : []).slice(0, Math.floor(MAX_LINE_SEGMENTS/12))) {
      if (segments.length + 12 > MAX_LINE_SEGMENTS) break;
      if (!box || !finiteVector(box.center,3) || !finiteVector(box.half_extents,3) ||
          !finiteVector(box.rotation,2) || !finiteVector(box.color,3) ||
          !Number.isFinite(box.alpha) || box.alpha < 0 || box.alpha > 1 ||
          box.center.some(v => Math.abs(v)>10_000_000) ||
          box.half_extents.some(v => v<=0 || v>10_000) ||
          box.color.some(v => v<0 || v>8)) continue;
      const [c,s]=box.rotation;
      if (Math.abs(c*c+s*s-1)>0.001) continue;
      const [cx,cy,cz]=box.center, [hx,hy,hz]=box.half_extents, corners=[];
      for (const x of [-1,1]) for (const y of [-1,1]) for (const z of [-1,1]) {
        const lx=x*hx, lz=z*hz;
        corners.push([cx+c*lx-s*lz, cy+y*hy, cz+s*lx+c*lz]);
      }
      for (const [a,b] of BOX_EDGES)
        segments.push([...corners[a],...corners[b],...box.color,box.alpha*0.9]);
    }
    if (segments.length) this.layers.set(layer, segments); else this.layers.delete(layer);
    return true;
  }
  packed() {
    const values=[];
    for (const lines of this.layers.values()) for (const line of lines) {
      if (values.length / 10 >= MAX_LINE_SEGMENTS) return new Float32Array(values);
      values.push(...line);
    }
    return new Float32Array(values);
  }
}
