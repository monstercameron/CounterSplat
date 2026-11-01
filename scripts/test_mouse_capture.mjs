import assert from 'node:assert/strict';
import { MouseCapture } from '../preview/mouse_capture.js';

let calls = 0, exits = 0;
const canvas = { requestPointerLock() { calls++; return Promise.resolve(); } };
const document = { pointerLockElement:null, exitPointerLock() { exits++; } };
const capture = new MouseCapture(canvas, document);
capture.request(); assert.equal(calls, 1); assert.equal(capture.requested, true);
document.pointerLockElement = canvas; assert.equal(capture.changed(true), 'locked');
capture.release(); assert.equal(exits, 1);
document.pointerLockElement = null; assert.equal(capture.changed(true), 'released');
// If a request completes after tilde, the late capture is released immediately.
capture.request(); capture.release(); document.pointerLockElement = canvas;
assert.equal(capture.changed(true), 'cancelled'); assert.equal(exits, 2);
document.pointerLockElement = null; capture.changed(true);
// Unrequested browser release (such as Escape) is distinguishable from tilde.
capture.request(); document.pointerLockElement = canvas; capture.changed(true);
document.pointerLockElement = null; assert.equal(capture.changed(true), 'unexpected');
const unavailable = new MouseCapture({}, document); unavailable.request();
assert.equal(unavailable.failed, true); assert.equal(unavailable.requested, false);
canvas.requestPointerLock = () => Promise.reject(new Error('denied'));
capture.request(); await Promise.resolve(); assert.equal(capture.failed, true);
// A rejected request is retryable after the next user gesture.
canvas.requestPointerLock = () => Promise.resolve(); capture.request();
document.pointerLockElement = canvas; assert.equal(capture.changed(true), 'locked');
console.log('Mouse capture and unlock tests passed.');
