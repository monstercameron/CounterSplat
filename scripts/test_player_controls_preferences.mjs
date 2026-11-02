import assert from 'node:assert/strict';
import {PLAYER_CONTROLS_KEY, LEGACY_PLAYER_CONTROLS_KEY, readControlsPreference} from '../preview/player_controls_preferences.js';
assert.equal(PLAYER_CONTROLS_KEY, 'countersplat.player-controls.v2');
assert.equal(LEGACY_PLAYER_CONTROLS_KEY, 'countersplat.player-controls.v1');
const old = {sprint_multiplier:3, move_speed:4, mouse_look:false, bindings:{sprint:'ShiftRight'}};
assert.deepEqual(readControlsPreference(null, JSON.stringify(old), 1.6),
  {settings:{...old,sprint_multiplier:1.6},migrated:true});
assert.deepEqual(readControlsPreference(null, JSON.stringify({...old,sprint_multiplier:2.2}), 1.6),
  {settings:{...old,sprint_multiplier:2.2},migrated:true});
assert.deepEqual(readControlsPreference(JSON.stringify(old), JSON.stringify({...old,sprint_multiplier:1.8}), 1.6),
  {settings:old,migrated:false});
assert.equal(readControlsPreference(null,null,1.6), null);
assert.throws(() => readControlsPreference(null,'[]',1.6));
assert.throws(() => readControlsPreference(null,'x'.repeat(16385),1.6));
assert.throws(() => readControlsPreference('{broken',null,1.6));
assert.equal(readControlsPreference(null,JSON.stringify(old),undefined).settings.sprint_multiplier,3);
console.log('Control migration updates the old sprint default while preserving custom preferences.');
