import assert from 'node:assert/strict';
import {layoutKey, migrateGameplayLayout} from '../preview/ui_layout_preferences.js';
assert.equal(layoutKey('gameplay'),'countersplat.ui-layout.gameplay.v2');
assert.equal(layoutKey('selector'),'countersplat.ui-layout.selector.v1');
assert.equal(layoutKey('menu'),'countersplat.ui-layout.menu.v1');
const prior={version:2, offsets:{ecs_panel:[100,20],stats_panel:[-20,2],weapon_hud:[9,7]},
  sizes:{ecs_panel:[500,600],other_widget:[30,40]}};
assert.deepEqual(JSON.parse(migrateGameplayLayout(JSON.stringify(prior))),
  {version:2,offsets:{weapon_hud:[9,7]},sizes:{other_widget:[30,40]}});
assert.deepEqual(JSON.parse(migrateGameplayLayout(JSON.stringify({scene_panel:[3,4],weapon_hud:[5,6]}))),
  {weapon_hud:[5,6]});
for (const value of [null,'invalid','[]','{"version":3,"offsets":{},"sizes":{}}'])
  assert.equal(migrateGameplayLayout(value),null);
console.log('Gameplay layout migration preserves unrelated placements and resets tool panels.');
