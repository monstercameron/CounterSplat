// Track right-button transitions from Pointer Events, including button chords.
// Browsers report additional mouse buttons as pointermove while another is held.
export class GameplayMouseButtons {
  constructor(emit) { this.emit = emit; this.rightDown = false; this.blocked = false; }
  update(event, allowPress) {
    if (event.pointerType !== 'mouse') return false;
    const down = (event.buttons & 2) !== 0;
    if (!down) {
      const wasDown = this.rightDown;
      this.rightDown = false; this.blocked = false;
      if (wasDown) this.emit(false);
      return false;
    }
    // A fresh pointerdown is a new physical press, even after a focus reset.
    if (event.type === 'pointerdown' && event.button === 2 && !this.rightDown) this.blocked = false;
    if (!this.rightDown && !this.blocked && event.button === 2) {
      if (!allowPress) { this.blocked = true; return false; }
      this.rightDown = true; this.emit(true); return true;
    }
    return false;
  }
  reset() {
    // Don't reactivate a held button after pause, focus loss, or pointer cancel.
    this.blocked = this.blocked || this.rightDown;
    this.rightDown = false;
  }
}
