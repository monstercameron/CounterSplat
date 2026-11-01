// Opt-in worker round-trip diagnostics. No game state is stored here.
import {summarizeSamples} from './frame_profile.js';

export class WorkerLatency {
  constructor({capacity=600, pendingLimit=512, now=() => performance.now()} = {}) {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 3600 ||
        !Number.isInteger(pendingLimit) || pendingLimit < 1 || pendingLimit > 4096)
      throw new RangeError('Invalid worker latency bounds');
    this.capacity = capacity; this.pendingLimit = pendingLimit; this.now = now;
    this.pending = new Map(); this.operations = new Map(); this.evicted = 0;
  }
  start(id, operation, batchSize) {
    if (!Number.isSafeInteger(id) || typeof operation !== 'string') return;
    if (!this.pending.has(id) && this.pending.size >= this.pendingLimit) {
      this.pending.delete(this.pending.keys().next().value); this.evicted++;
    }
    this.pending.set(id, {operation, batchSize, started:this.now()});
  }
  finish(id, workerMs, failed=false) {
    const entry = this.pending.get(id);
    if (!entry) return;
    this.pending.delete(id);
    const elapsed = this.now() - entry.started;
    if (!Number.isFinite(elapsed) || elapsed < 0) return;
    let data = this.operations.get(entry.operation);
    if (!data) {
      data = {roundTrip:[], worker:[], batchSize:[], completed:0, errors:0};
      this.operations.set(entry.operation, data);
    }
    data.completed++; if (failed) data.errors++;
    if (Number.isSafeInteger(entry.batchSize) && entry.batchSize > 0) {
      data.batchSize.push(entry.batchSize);
      if (data.batchSize.length > this.capacity) data.batchSize.shift();
    }
    data.roundTrip.push(elapsed);
    if (data.roundTrip.length > this.capacity) data.roundTrip.shift();
    if (Number.isFinite(workerMs) && workerMs >= 0) {
      data.worker.push(workerMs);
      if (data.worker.length > this.capacity) data.worker.shift();
    }
  }
  snapshot() {
    return {pending:this.pending.size, evicted:this.evicted,
      operations:Object.fromEntries([...this.operations].map(([name, data]) => [name, {
        completed:data.completed, errors:data.errors,
        round_trip_ms:summarizeSamples(data.roundTrip), worker_ms:summarizeSamples(data.worker),
        batch_size:summarizeSamples(data.batchSize)
      }]))};
  }
}
