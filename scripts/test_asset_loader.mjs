import assert from 'node:assert/strict';
import {loadPolicy, levelPolicies, planScripts, LoadingProgress, describeLevelAssets, localSceneUrl } from '../preview/asset_loader.js';
assert.equal(loadPolicy(), 'preload');
assert.equal(levelPolicies().splat_chunks, 'streamable');
assert.equal(levelPolicies({splat_chunks:'preload'}).splat_chunks, 'preload');
assert.throws(() => levelPolicies({chunk_metadata:'streamable'}));
assert.throws(() => loadPolicy('unknown'));
const base = new URL('http://localhost/assets/scenes/gameplay.json');
const scene = {resources:{scripts:{main:{language:'lua',uri:'../scripts/main.lua',load:'preload',autoload:true},
  optional:{language:'lua',uri:'../scripts/optional.lua',load:'streamable'}}}};
assert.deepEqual(planScripts(scene, base, base.origin).map(s => s.id), ['main']);
assert.throws(() => planScripts({resources:{scripts:{bad:{language:'lua',uri:'x',load:'streamable',autoload:true}}}}, base, base.origin));
// The scene's explicit order controls execution even when catalog keys differ.
const orderedScene = {resources:{scripts:{late:{language:'lua',uri:'late.lua',load:'preload'},
  early:{language:'lua',uri:'early.lua',load:'preload'}, auto:{language:'lua',uri:'auto.lua',load:'preload',autoload:true}},
  preview:{browser_scripts:['early','late','early']}}};
assert.deepEqual(planScripts(orderedScene,base,base.origin).filter(item=>item.execute).map(item=>item.id), ['early','late','auto']);

const reports = [];
const tracker = new LoadingProgress((...args) => reports.push(args));
tracker.update('Metadata',0.1,0.5,1,2);
tracker.update('Metadata',0.1,0.5,2,2);
tracker.update('Cached',0,0.1,1,1);
assert.ok(Math.abs(reports[0][1] - 0.3) < 1e-9);
assert.deepEqual(reports.slice(1).map(report => report[1]), [0.5, 0.5]);
assert.throws(() => tracker.update('Bad',0,1,2,1));
console.log('Asset loading policies and progress tests passed.');

const example = {id:'1234abcd', name:'Example', format:'streamed_sog', assets:{splat_chunks:'streamable'}};
const inventory = {scene_id:'1234abcd', files:[{path:'lod-meta.json'},{path:'scene.voxel.bin'}]};
const described = describeLevelAssets(example, 2, [{means:{files:['means_l.webp','means_u.webp']}}], ['0_0/meta.json'],
  {viewerSettings:true, inventory, source:true});
assert.equal(described.lod_count, 2);
assert.deepEqual(described.files.find(f => f.path === '0_0/means_l.webp'), {path:'0_0/means_l.webp',kind:'splat_data',load:'streamable'});
assert.equal(described.files.find(f => f.path === 'scene.voxel.bin').kind, 'collision');
assert.equal(described.files.filter(f => f.path === 'lod-meta.json').length, 1);
assert.throws(() => describeLevelAssets(example,2,[{means:{files:['../secret']}}],['0_0/meta.json']));
assert.throws(() => describeLevelAssets(example,2,[{means:{files:[null]}}],['0_0/meta.json']));

assert.equal(localSceneUrl('gameplay.json', base.origin).href, 'http://localhost/assets/scenes/gameplay.json');
assert.throws(() => localSceneUrl('../secret.json', base.origin));
assert.throws(() => localSceneUrl('https://evil.test/a.json', base.origin));
