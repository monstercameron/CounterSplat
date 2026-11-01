// Browser-owned pointer lock, including cancellation of an in-flight lock request.
export class MouseCapture {
  constructor(canvas, document) {
    this.canvas = canvas; this.document = document;
    this.requested = false; this.failed = false;
    this.hadLock = false; this.expectedUnlock = false; this.sequence = 0;
  }
  get locked() { return this.document.pointerLockElement === this.canvas; }
  request() {
    const sequence = ++this.sequence;
    this.requested = true;
    if (typeof this.canvas.requestPointerLock !== 'function') { this.error(); return; }
    try {
      // This call must remain synchronous with the user's click.
      const promise = this.canvas.requestPointerLock();
      if (promise && typeof promise.catch === 'function') {
        promise.catch(() => { if (this.sequence === sequence) this.error(); });
      }
    } catch (_) { this.error(); }
  }
  error() { this.failed = true; this.requested = false; }
  release() {
    ++this.sequence;
    this.requested = false;
    if (this.locked) { this.expectedUnlock = true; this.document.exitPointerLock(); }
  }
  // Call on pointerlockchange; a late response must not recapture after pause or unlock.
  changed(allowCapture) {
    if (this.locked) {
      if (!this.requested || !allowCapture) {
        this.expectedUnlock = true; this.document.exitPointerLock(); return 'cancelled';
      }
      this.hadLock = true; this.failed = false;
      return 'locked';
    }
    this.requested = false;
    if (!this.hadLock) { this.expectedUnlock = false; return 'unlocked'; }
    this.hadLock = false;
    if (this.expectedUnlock) { this.expectedUnlock = false; return 'released'; }
    return 'unexpected';
  }
}
