import assert from 'node:assert/strict';
import {FrameStackCapture, summarizeFrameStacks, medianGpuSample} from '../preview/frame_stack_profile.js';

const trace = {
  resources:['local.js'],
  frames:[{name:'tick', resourceId:0, line:1}, {name:'render', resourceId:0, line:10},
    {name:'script', resourceId:0, line:20}],
  stacks:[{frameId:0}, {frameId:1, parentId:0}, {frameId:2, parentId:0}],
  samples:[{timestamp:.5, stackId:1}, {timestamp:3.2, stackId:1},
    {timestamp:3.8, stackId:1}, {timestamp:6.5, stackId:2}, {timestamp:10, stackId:2}]
};
const frames = [
  {start:0,end:2,submitted:true,details:{render_cpu_ms:1},animation_timestamp_ms:100},
  {start:3,end:7,submitted:true,details:{render_cpu_ms:2},animation_timestamp_ms:200},
  {start:8,end:14,submitted:false,details:{render_cpu_ms:3},animation_timestamp_ms:300}
];
const result = summarizeFrameStacks(trace, frames, 1, 16);
assert.equal(result.actual_interval_ms, 16);
assert.equal(result.captured_frames, 3);
assert.equal(result.samples_within_frames, 5);
assert.equal(result.selected_frame.index, 1);
assert.equal(result.selected_frame.sample_count, 3);
assert.equal(result.selected_frame.stacks[0].count, 2);
assert.deepEqual(result.selected_frame.stacks[0].frames.map(frame => frame.name), ['tick','render']);
const matchingGpuFrame = summarizeFrameStacks(trace, frames, 1, 16, 100);
assert.equal(matchingGpuFrame.selected_frame.index, 0);
assert.equal(matchingGpuFrame.selected_frame.sample_count, 1);
assert.equal(summarizeFrameStacks(trace, frames, 1, 16, 300).selected_frame.index, 1);
const gpuSamples = [{gpu_ms:3,frame_timestamp_ms:1}, {gpu_ms:1,frame_timestamp_ms:2},
  {gpu_ms:2,frame_timestamp_ms:3}, {gpu_ms:NaN,frame_timestamp_ms:4}];
assert.equal(medianGpuSample(gpuSamples).frame_timestamp_ms, 3);
assert.equal(gpuSamples[0].gpu_ms, 3, 'selection does not reorder samples');
assert.equal(medianGpuSample([]), null);
assert.equal(summarizeFrameStacks({resources:[], frames:[], stacks:[], samples:[]}, [], 1).selected_frame, null);

let now = 0, completed = null;
const luaProfile = {systems:{camera:{count:0,total_ms:0}}, operations:{}};
const lua = {profileReport:() => luaProfile};
const fakeTrace = {resources:['x'], frames:[{name:'tick',resourceId:0}],
  stacks:[{frameId:0}], samples:[{timestamp:.5,stackId:0},{timestamp:1.5,stackId:0}]};
const capture = new FrameStackCapture((summary, raw) => {completed={summary,raw};}, {
  warmFrames:1, captureFrames:2, detailEvery:1, now:() => now,
  makeProfiler:() => ({sampleInterval:16, stop:async () => fakeTrace})
});
for (let i=0; i<2; i++) {
  const frame = capture.beginFrame(true, lua);
  now++; luaProfile.systems.camera.count++; luaProfile.systems.camera.total_ms += .25;
  capture.endFrame(frame, true, .2, .5, {lua_systems_ms:.5}, lua, [['ui_render',.1]], 100+i);
}
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(completed.summary.actual_interval_ms, 16);
assert.equal(completed.summary.selected_frame.lua.systems[0].count, 1);
assert.equal(completed.summary.selected_frame.lua.systems[0].total_ms, .25);
assert.deepEqual(completed.summary.selected_frame.rust, [['ui_render',.1]]);
assert.equal(completed.raw.frames.length, 2);
assert.equal(completed.raw.frames[0].animation_timestamp_ms, 100);
assert.equal(capture.beginFrame(true, lua), null);

let stopped = 0;
const cancelled = new FrameStackCapture(() => {throw new Error('cancelled capture must not publish');}, {
  warmFrames:1, now:() => now, makeProfiler:() => ({stop:async () => {stopped++; return fakeTrace;}})
});
assert.ok(cancelled.beginFrame(true, lua));
assert.equal(cancelled.beginFrame(false, null), null);
assert.equal(stopped, 1);
assert.equal(cancelled.frames.length, 0);

let failure;
const unsupported = new FrameStackCapture(result => {failure=result;}, {
  warmFrames:1, makeProfiler:() => {throw new Error('unavailable');}
});
assert.equal(unsupported.beginFrame(true, lua), null);
assert.match(failure.error, /unavailable/);
console.log('Frame stack capture tests passed.');
