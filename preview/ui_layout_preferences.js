// Layout positions are versioned separately from scene content. A new gameplay
// arrangement resets tool positions while retaining unrelated HUD placements.
const gameplayTools = new Set([
  'scene_panel', 'stats_panel', 'sky_panel', 'controls_panel', 'level_panel',
  'entity_inspector', 'ecs_panel', 'camera_panel', 'image_panel',
]);
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const withoutTools = value => Object.fromEntries(Object.entries(value).filter(([id]) => !gameplayTools.has(id)));

export function layoutKey(scene) {
  return `countersplat.ui-layout.${scene}.${scene === 'gameplay' ? 'v2' : 'v1'}`;
}

// Both the old flat offsets format and the current offset/size format are in use.
// Leave the old storage entry untouched, so existing preferences are recoverable.
export function migrateGameplayLayout(raw) {
  if (typeof raw !== 'string') return null;
  try {
    const saved = JSON.parse(raw);
    if (!object(saved)) return null;
    if (typeof saved.version === 'number') {
      if (saved.version !== 2 || !object(saved.offsets) || !object(saved.sizes)) return null;
      return JSON.stringify({...saved, offsets:withoutTools(saved.offsets), sizes:withoutTools(saved.sizes)});
    }
    return JSON.stringify(withoutTools(saved));
  } catch { return null; }
}
