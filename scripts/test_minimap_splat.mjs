import assert from 'node:assert/strict';
import {decodeSplatCenters, rasterizeSplatSlice, boundedVoxelRadius, selectSplatMapFiles} from '../preview/minimap_splat.js';
const meta={count:1,means:{mins:[Math.log(2),-Math.log(3),Math.log(4)],maxs:[Math.log(2),-Math.log(3),Math.log(4)]}};
const points=decodeSplatCenters(meta,new Uint8Array(4),new Uint8Array(4),true);
assert(Math.abs(points[0]+1)<1e-5 && Math.abs(points[1]-2)<1e-5 && Math.abs(points[2]-3)<1e-5);
const northUp=rasterizeSplatSlice([points,new Float32Array([-1,5,3])],[-1,0,3],2,4,.4,16);
assert.equal(northUp[8*16+8],255);
assert.equal(northUp.reduce((a,b)=>a+(b===255),0),1,'only the requested height is projected');
assert.equal(northUp.reduce((a,b)=>a+(b>0),0),9,'one-pixel footprints make sparse observations legible');
assert.throws(()=>decodeSplatCenters(meta,new Uint8Array(3),new Uint8Array(4),true));
assert.throws(()=>decodeSplatCenters({...meta,count:3_000_001},new Uint8Array(4),new Uint8Array(4),true));
console.log('Splat map inverse-log decoding, coordinates, and slice passed.');

assert.equal(boundedVoxelRadius(12,.4,.05),12);
const bounded=boundedVoxelRadius(75,2,.05);
assert(bounded<75 && (Math.ceil(2*bounded/.05)+2)**2*(Math.ceil(2/.05)+2)<16_000_000);

assert.deepEqual(selectSplatMapFiles(null),['meta.json']);
assert.deepEqual(selectSplatMapFiles({lodLevels:3,counts:[5_000_000,2_000_000,1_000_000],
  filenames:['0_0/meta.json','1_0/meta.json','1_1/meta.json','2_0/meta.json','1_0/../../other']}),
  ['1_0/meta.json','1_1/meta.json']);
assert.throws(()=>selectSplatMapFiles({lodLevels:4,filenames:['../../outside']}));
