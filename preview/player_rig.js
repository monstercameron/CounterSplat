// Browser storage adapter. Lua owns how rig defaults and saved values affect gameplay.
const keyFor = id => `countersplat.player-rig.v1.${id}`;
export function publishPlayerRigData(game, level, storage = localStorage) {
  try {
    const setResource = (name, value) => game.ecs_operation('gameplay', JSON.stringify({op:'set_resource', name, value}));
    if (level.player_units_per_meter != null) setResource('player.level_units_per_meter', level.player_units_per_meter);
    const raw = storage.getItem(keyFor(level.id));
    if (raw) {
      if (raw.length > 4096) throw new Error('Saved player rig is too large.');
      setResource('player.saved_rig', JSON.parse(raw));
    }
  } catch (error) { console.warn('Could not restore player rig:', error); }
}
export function savePlayerRig(level, rig, storage = localStorage) {
  try { storage.setItem(keyFor(level.id), JSON.stringify(rig)); }
  catch (error) { console.warn('Could not save player rig:', error); }
}
