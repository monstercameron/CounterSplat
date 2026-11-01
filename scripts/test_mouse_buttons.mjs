import assert from 'node:assert/strict';
import { GameplayMouseButtons } from '../preview/mouse_buttons.js';

const events = [];
const buttons = new GameplayMouseButtons(pressed => events.push(pressed));
const pointer = (type, button, mask, pointerType = 'mouse') => ({type, button, buttons:mask, pointerType});

// A UI press never turns into zoom when dragged out of the UI.
buttons.update(pointer('pointerdown', 2, 2), false);
buttons.update(pointer('pointermove', -1, 2), true);
buttons.update(pointer('pointerup', 2, 0), false);
assert.deepEqual(events, []);
// Normal right-button hold/release and continued pointer movement.
assert.equal(buttons.update(pointer('pointerdown', 2, 2), true), true);
buttons.update(pointer('pointermove', -1, 2), true);
buttons.update(pointer('pointerup', 2, 0), false);
assert.deepEqual(events, [true, false]);
// With another mouse button held, Pointer Events report changes as pointermove.
buttons.update(pointer('pointerdown', 0, 1), true);
buttons.update(pointer('pointermove', 2, 3), true);
buttons.update(pointer('pointermove', 2, 1), true);
assert.deepEqual(events, [true, false, true, false]);
// A focus reset cancels the hold; movement cannot reactivate it until a new press.
buttons.update(pointer('pointerdown', 2, 2), true);
buttons.reset();
buttons.update(pointer('pointermove', -1, 2), true);
buttons.update(pointer('pointerup', 2, 0), false);
buttons.update(pointer('pointerdown', 2, 2), true);
buttons.update(pointer('pointerup', 2, 0), false);
assert.deepEqual(events, [true, false, true, false, true, true, false]);
buttons.update(pointer('pointerdown', 2, 2, 'touch'), true);
assert.equal(buttons.rightDown, false);
console.log('Gameplay mouse button, UI gating, chord, and reset tests passed.');
