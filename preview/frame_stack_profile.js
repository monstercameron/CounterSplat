// Opt-in local CPU sampling. GPU execution and other threads are not measured.
const PROFILE_GROUPS = ['systems', 'operations'];

function snapshotLua(profile) {
  const result = {};
  for (const group of PROFILE_GROUPS) {
    result[group] = {};
    for (const [name, value] of Object.entries(profile?.[group] || {}))
      result[group][name] = {count:value.count, total_ms:value.total_ms};
  }
  return result;
}

function luaDelta(before, after) {
  const result = {};
  for (const group of PROFILE_GROUPS) {
    result[group] = Object.entries(after[group]).map(([name, value]) => {
      const old = before[group][name] || {count:0, total_ms:0};
      return {name, count:value.count-old.count, total_ms:Math.max(0, value.total_ms-old.total_ms)};
    }).filter(value => value.count > 0).sort((a,b) => b.total_ms-a.total_ms);
  }
  return result;
}

function stackFrames(trace, sample) {
  const result = [], seen = new Set();
  let id = sample.stackId;
  while (Number.isInteger(id) && id >= 0 && id < trace.stacks.length && !seen.has(id)) {
    seen.add(id);
    const stack = trace.stacks[id], frame = trace.frames[stack.frameId];
    if (!frame) break;
    result.push({name:frame.name || '(anonymous)', resource:trace.resources[frame.resourceId] || null,
      line:frame.line ?? null, column:frame.column ?? null});
    id = stack.parentId;
  }
  return result.reverse();
}

function topStacks(trace, samples) {
  const counts = new Map();
  for (const sample of samples) {
    const frames = stackFrames(trace, sample);
    if (!frames.length) continue;
    const key = JSON.stringify(frames);
    const existing = counts.get(key);
    if (existing) existing.count++;
    else counts.set(key, {count:1, frames});
  }
  return [...counts.values()].sort((a,b) => b.count-a.count).slice(0, 20);
}

// The selected frame is a submitted, representative CPU frame with sampled
// stacks when available. Sample counts are not exact per-function durations.
export function summarizeFrameStacks(trace, frames, requestedIntervalMs, actualIntervalMs = null) {
  const samplesByFrame = frames.map(frame =>
    trace.samples.filter(sample => sample.timestamp >= frame.start && sample.timestamp < frame.end));
  const submitted = frames.filter(frame => frame.submitted);
  const durations = (submitted.length ? submitted : frames).map(frame => frame.end-frame.start).sort((a,b) => a-b);
  const median = durations.length ? durations[Math.floor(durations.length/2)] : 0;
  let candidates = frames.map((frame,index) => ({frame,index,samples:samplesByFrame[index]}))
    .filter(item => item.frame.details && item.frame.submitted && item.samples.length >= 2);
  if (!candidates.length) candidates = frames.map((frame,index) => ({frame,index,samples:samplesByFrame[index]}))
    .filter(item => item.frame.details && item.frame.submitted && item.samples.length > 0);
  if (!candidates.length) candidates = frames.map((frame,index) => ({frame,index,samples:samplesByFrame[index]}))
    .filter(item => item.frame.details);
  candidates.sort((a,b) => Math.abs((a.frame.end-a.frame.start)-median)-Math.abs((b.frame.end-b.frame.start)-median));
  const selected = candidates[0];
  const frameSamples = samplesByFrame.flat();
  return {
    requested_interval_ms:requestedIntervalMs, actual_interval_ms:actualIntervalMs,
    captured_frames:frames.length,
    captured_samples:trace.samples.length,
    samples_within_frames:frameSamples.length,
    median_submitted_cpu_ms:median,
    selected_frame:selected ? {
      index:selected.index, cpu_ms:selected.frame.end-selected.frame.start, submitted:selected.frame.submitted,
      ...selected.frame.details, sample_count:selected.samples.length, stacks:topStacks(trace, selected.samples)
    } : null,
    aggregate_stacks:topStacks(trace, frameSamples)
  };
}

export class FrameStackCapture {
  constructor(onComplete, {warmFrames=120, captureFrames=180, detailEvery=10,
    now=() => performance.now(), makeProfiler=() => new Profiler({sampleInterval:1, maxBufferSize:10000})} = {}) {
    this.onComplete = onComplete; this.warmFrames = warmFrames; this.captureFrames = captureFrames;
    this.detailEvery = detailEvery; this.now = now; this.makeProfiler = makeProfiler;
    this.warm = 0; this.frames = []; this.profiler = null; this.done = false;
  }

  beginFrame(gameplay, lua) {
    if (this.done) return null;
    if (!gameplay || !lua) {
      if (this.profiler) {
        this.profiler.stop().catch(() => {});
        this.profiler = null; this.frames = [];
      }
      this.warm = 0;
      return null;
    }
    if (!this.profiler) {
      if (++this.warm < this.warmFrames) return null;
      try { this.profiler = this.makeProfiler(); }
      catch (error) { this.done = true; this.onComplete({error:String(error)}, null); return null; }
    }
    const detail = this.frames.length % this.detailEvery === 0;
    const before = detail ? snapshotLua(lua.profileReport()) : null;
    return {start:this.now(), before};
  }

  endFrame(frame, submitted, renderMs, scriptsMs, stages, lua, rustTiming = null) {
    if (!frame || !this.profiler || this.done) return;
    const end = this.now();
    const details = frame.before ? {
      render_cpu_ms:renderMs, scripts_cpu_ms:scriptsMs, stages:{...stages}, rust:rustTiming,
      lua:luaDelta(frame.before, snapshotLua(lua.profileReport()))
    } : null;
    this.frames.push({start:frame.start, end, submitted, details});
    if (this.frames.length < this.captureFrames) return;
    const profiler = this.profiler, frames = this.frames;
    this.profiler = null; this.done = true;
    profiler.stop().then(trace => this.onComplete(summarizeFrameStacks(trace, frames, 1, profiler.sampleInterval ?? null), {trace, frames}))
      .catch(error => this.onComplete({error:String(error)}, null));
  }
}
