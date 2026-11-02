// Version 1 used a 3x sprint default. Preserve all custom values and only
// update that old default when a player first migrates to version 2.
export const PLAYER_CONTROLS_KEY = 'countersplat.player-controls.v2';
export const LEGACY_PLAYER_CONTROLS_KEY = 'countersplat.player-controls.v1';
export function readControlsPreference(currentRaw, legacyRaw, sceneSprintDefault) {
  const migrated = currentRaw === null && legacyRaw !== null;
  const raw = migrated ? legacyRaw : currentRaw;
  if (raw === null) return null;
  if (typeof raw !== 'string' || raw.length > 16384) throw new Error('Saved controls are too large or invalid.');
  const settings = JSON.parse(raw);
  if (!settings || typeof settings !== 'object' || Array.isArray(settings)) throw new Error('Saved controls must be an object.');
  if (migrated && settings.sprint_multiplier === 3 && Number.isFinite(sceneSprintDefault)) {
    settings.sprint_multiplier = sceneSprintDefault;
  }
  return {settings, migrated};
}
