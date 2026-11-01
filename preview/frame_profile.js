// Opt-in browser-side measurements. Times are CPU wall time, not GPU timestamps.
export function summarizeSamples(samples) {
  if (!samples.length) return null;
  const values = [...samples].sort((a,b) => a-b);
  const mean = values.reduce((sum,value) => sum+value, 0) / values.length;
  const percentile = p => values[Math.ceil((values.length - 1) * p)];
  return {samples:values.length, mean, p50:percentile(0.5), p95:percentile(0.95),
    p99:percentile(0.99), max:values[values.length-1]};
}
export class FrameProfiler {
  constructor(capacity=600) {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 3600)
      throw new RangeError('Frame profile capacity must be an integer between 1 and 3600');
    this.capacity = capacity;
    this.reset();
  }
  reset() {
    this.lastFrame = this.lastSubmit = null;
    this.samples = {animation_ms:[], submit_ms:[], render_cpu_ms:[], scripts_cpu_ms:[], callback_cpu_ms:[],
      event_dispatch_ms:[], lua_systems_ms:[], tick_events_ms:[], output_ms:[], voxel_ms:[], stream_ms:[]};
  }
  push(name, value) {
    if (!Number.isFinite(value) || value < 0) return;
    const samples = this.samples[name];
    samples.push(value);
    if (samples.length > this.capacity) samples.shift();
  }
  record(timestamp, submitted, renderMs, scriptsMs, callbackMs, stages = {}) {
    if (!Number.isFinite(timestamp)) return;
    if (this.lastFrame !== null) this.push('animation_ms', timestamp-this.lastFrame);
    this.lastFrame = timestamp;
    if (submitted) {
      if (this.lastSubmit !== null) this.push('submit_ms', timestamp-this.lastSubmit);
      this.lastSubmit = timestamp;
    }
    this.push('render_cpu_ms', renderMs);
    this.push('scripts_cpu_ms', scriptsMs);
    this.push('callback_cpu_ms', callbackMs);
    for (const name of ['event_dispatch_ms','lua_systems_ms','tick_events_ms','output_ms','voxel_ms','stream_ms'])
      this.push(name, stages[name] || 0);
  }
  snapshot(state, lua) {
    const metrics = Object.fromEntries(Object.entries(this.samples).map(([key, values]) => [key, summarizeSamples(values)]));
    const submitted = Number(state?.submitted_frames) || 0, busy = Number(state?.busy_skips) || 0;
    return {...metrics, gpu_busy_skips:busy, submitted_frames:submitted,
      busy_fraction:busy+submitted ? busy/(busy+submitted) : 0, lua};
  }
}
