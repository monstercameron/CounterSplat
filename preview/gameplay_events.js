// Channel order is part of the Lua event contract. Input can be dispatched
// before drawing; later channels remain in the original post-draw pass.
export const GAMEPLAY_INPUT_CHANNELS = Object.freeze([
  'input.key', 'input.mouse_delta', 'input.mouse_button', 'input.reset'
]);
export const GAMEPLAY_EVENT_CHANNELS = Object.freeze([
  'ui.action', 'editor.entity_selected', 'physics.ready', 'physics.ground_result',
  'physics.clearance_result', 'physics.trace_result', 'physics.motion_resolved', 'physics.motion_error',
  'physics.collision', 'physics.error', 'weapon.trigger', 'weapon.aim', 'weapon.reload',
  'weapon.equip', 'weapon.mode', 'weapon.hit', 'weapon.die', 'weapon.posture'
]);
