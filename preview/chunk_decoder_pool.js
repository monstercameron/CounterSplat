// Bounded CPU workers for streamed chunks. There is no waiting queue: the
// renderer chooses fresh priorities whenever a worker becomes available.
export function chunkDecoderCount(logicalCores, configured = 'auto') {
  if (configured !== 'auto') {
    if (!Number.isInteger(configured) || configured < 1 || configured > 4)
      throw new Error('Chunk decoder workers must be auto or an integer from 1 to 4.');
    return configured;
  }
  if (!Number.isFinite(logicalCores) || logicalCores < 1) return 2;
  if (logicalCores <= 2) return 1;
  if (logicalCores <= 4) return 2;
  if (logicalCores <= 8) return 3;
  return 4;
}

export class ChunkDecoderPool {
  constructor({size, createWorker, onResult, onFailure, onFatal}) {
    if (!Number.isInteger(size) || size < 1 || size > 4) throw new Error('Invalid chunk worker count.');
    this.createWorker = createWorker;
    this.onResult = onResult;
    this.onFailure = onFailure;
    this.onFatal = onFatal;
    this.closed = false;
    this.slots = [];
    try {
      for (let i = 0; i < size; i++) {
        const slot = {worker:null, index:null};
        this.slots.push(slot);
        this.start(slot);
      }
    } catch (error) {
      this.stop();
      throw error;
    }
  }

  start(slot) {
    const worker = this.createWorker();
    slot.worker = worker;
    worker.onmessage = ({data}) => {
      if (this.closed || slot.worker !== worker || slot.index === null) return;
      const index = slot.index;
      slot.index = null;
      if (!data || data.index !== index) this.onFailure(index, 'Decoder returned an unexpected chunk.');
      else if (!data.ok) this.onFailure(index, data.error || 'Chunk decode failed.');
      else this.onResult(data);
    };
    worker.onerror = event => {
      if (this.closed || slot.worker !== worker) return;
      const index = slot.index;
      slot.index = null;
      slot.worker = null;
      worker.terminate();
      // Replace a worker that failed during a job. A replacement that fails
      // before receiving work is retired instead of spinning in a restart loop.
      if (index !== null) {
        try { this.start(slot); } catch (_) { /* other workers can continue */ }
        this.onFailure(index, event.message || 'Chunk worker failed.');
      }
      if (!this.slots.some(item => item.worker)) this.onFatal(event.message || 'All chunk workers failed.');
    };
  }

  get available() {
    return this.closed ? 0 : this.slots.filter(slot => slot.worker && slot.index === null).length;
  }

  get size() { return this.slots.filter(slot => slot.worker).length; }

  submit(task) {
    if (this.closed || this.slots.some(slot => slot.index === task.index)) return false;
    const slot = this.slots.find(item => item.worker && item.index === null);
    if (!slot) return false;
    slot.index = task.index;
    try {
      slot.worker.postMessage(task);
    } catch (error) {
      slot.index = null;
      this.onFailure(task.index, error);
    }
    return true;
  }

  stop() {
    this.closed = true;
    for (const slot of this.slots) {
      slot.worker?.terminate();
      slot.worker = null;
      slot.index = null;
    }
  }
}
