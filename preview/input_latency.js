// Opt-in timing from a browser mouse event to a later submitted frame.
import {summarizeSamples} from './frame_profile.js';

export class InputFrameLatency {
  constructor({capacity=600, pendingLimit=512, now=() => performance.now()} = {}) {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 3600 ||
        !Number.isInteger(pendingLimit) || pendingLimit < 1 || pendingLimit > 4096)
      throw new RangeError('Invalid input latency bounds');
    this.capacity=capacity; this.pendingLimit=pendingLimit; this.now=now;
    this.pending=[]; this.dispatched=[]; this.eventToSubmit=[]; this.dispatchToSubmit=[]; this.evicted=0;
  }
  recordInput() {
    const started=this.now();
    if (!Number.isFinite(started)) return;
    if (this.pending.length >= this.pendingLimit) { this.pending.shift(); this.evicted++; }
    this.pending.push(started);
  }
  markDispatched() {
    if (!this.pending.length) return;
    const dispatched=this.now();
    for (const started of this.pending) {
      if (!Number.isFinite(dispatched) || dispatched < started) continue;
      if (this.dispatched.length >= this.pendingLimit) { this.dispatched.shift(); this.evicted++; }
      this.dispatched.push({started, dispatched});
    }
    this.pending.length=0;
  }
  markSubmitted() {
    if (!this.dispatched.length) return;
    const submitted=this.now();
    for (const entry of this.dispatched) {
      if (!Number.isFinite(submitted) || submitted < entry.dispatched) continue;
      this.eventToSubmit.push(submitted-entry.started);
      this.dispatchToSubmit.push(submitted-entry.dispatched);
      if (this.eventToSubmit.length > this.capacity) this.eventToSubmit.shift();
      if (this.dispatchToSubmit.length > this.capacity) this.dispatchToSubmit.shift();
    }
    this.dispatched.length=0;
  }
  clearPending() { this.pending.length=0; this.dispatched.length=0; }
  reset() { this.clearPending(); this.eventToSubmit.length=0; this.dispatchToSubmit.length=0; this.evicted=0; }
  snapshot() {
    return {pending:this.pending.length, dispatched:this.dispatched.length, evicted:this.evicted,
      event_to_submit_ms:summarizeSamples(this.eventToSubmit),
      dispatch_to_submit_ms:summarizeSamples(this.dispatchToSubmit)};
  }
}
