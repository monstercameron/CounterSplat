import assert from 'node:assert/strict';
import {FrameProfiler} from '../preview/frame_profile.js';

const profile = new FrameProfiler(3);
for (let i=0; i<6; i++) profile.record(i*10, i%2===0, i, i*2, i*3, {lua_systems_ms:i});
const result = profile.snapshot({submitted_frames:3, busy_skips:1});
assert.equal(result.animation_ms.samples, 3);
assert.equal(result.animation_ms.mean, 10);
assert.equal(result.submit_ms.mean, 20);
assert.equal(result.lua_systems_ms.mean, 4);
assert.equal(result.busy_fraction, 0.25);
profile.record(5100, true, 1, 2, 3);
assert.equal(profile.snapshot(null).animation_ms.max, 5050, 'long stalls remain visible');
assert.throws(() => new FrameProfiler(Infinity), RangeError);
profile.reset();
assert.equal(profile.snapshot(null).animation_ms, null);
console.log('Bounded frame profiling tests passed.');
