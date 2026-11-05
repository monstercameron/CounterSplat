// Browser host: both the selector and gameplay render directly to the WebGPU canvas.
import init, { Selector, Viewer, set_indirect_draw, set_sort_compaction, set_footprint_cull, chunk_decoder_count, preload_level_chunks,
  DisplaySettings, display_viewport_size, MouseCapture, GameplayMouseButtons, GameplayFireButton, savedLayout, storeLayout,
  saveSetting, savedWeaponOptic, savePlayerRig, LoadingProgress, levelPolicies, loadPolicy, planScripts, localSceneUrl,
  projectResourceUrl, FrameProfiler, InputFrameLatency, gameplayInputChannels, gameplayEventChannels,
  loadLevelMetadata, FirstPersonFrames, SceneAudio, FrameStackCapture, MinimapRuntime } from './pkg/countersplat_web.js';
import { LuaScene, EventChannels } from './lua_runtime.js';

const canvas = document.getElementById('canvas');
const status = document.getElementById('status');
const message = document.getElementById('message');
const metrics = document.getElementById('metrics');
let sceneAudio = null;
// The selector click or a later key press unlocks audio without delaying input.
for (const name of ['pointerdown', 'keydown']) window.addEventListener(name, () => sceneAudio?.unlock(), {capture:true});
// Profiling is opt-in and publishes read-only diagnostics on the existing metrics node.
const profileOptions = new URLSearchParams(location.search);
const stackRequested = profileOptions.get('stack_profile') === '1';
// Rust-backed host objects are created once the module is initialised (see start).
let frameProfiler = null, inputLatency = null;
let stackCapture = null;
const publishStackProfile = (summary, raw) => {
  metrics.dataset.stackProfile = JSON.stringify(summary);
  if (raw) {
    metrics.dataset.stackTrace = JSON.stringify(raw);
    const download = document.getElementById('download-stack-trace');
    download.href = URL.createObjectURL(new Blob([JSON.stringify({summary, ...raw})], {type:'application/json'}));
    download.hidden = false;
  }
};

let profileState = null, lastProfileReport = 0;
const arrangeButton = document.getElementById('arrange');
const resetLayoutButton = document.getElementById('reset-layout');
let mouseCapture = null;
let active = null;
let displayPanel = null;
let displayViewport = {width:1, height:1};
let arrangeMode = false;
let mode = 'boot';
let generation = 0;
let lastStatus = 0;
let menuViewportOwner = null, menuViewportWidth = 0, menuViewportHeight = 0;
let stopped = false;
let selectorBytes, catalogBytes, sceneCatalogBytes, gameplayBytes, menuBytes, catalog, gameplayScene, menuScene;
let selectorUrl, sceneCatalogUrl, gameplayUrl, defaultGameplayUrl, defaultGameplayBytes, menuUrl, engineApi, gameplayScripts, menuScripts, luaFactory;
let gameplayLua = null, menuLua = null, lastScriptTime = null;
let pendingLightPresentation = null;
let gameplayMouse = null, gameplayFire = null;
let firstPersonFrames = null, minimapRuntime = null;
let inspectionPoseEnabled = false, lastInspectionPresentation = null, lastWeaponPresentation = null;
let dragLook = false, lastEscapeTime = -Infinity;
let activeWeaponId = null, gameplayEntityIds = null;
let loadingAbort = null, preloadedSceneAssets = new Map();
const MAX_PRELOAD_BYTES = 512 * 1024 * 1024;
let selectedId = '', selectedSceneId = '';
const previewImages = new Map(), previewPending = new Map();
let currentLevel = null;
let levelBase = null;
let skyDecoder = null;
// The gameplay viewer while it runs level collision (Rust, with its own worker).
let voxelGame = null;
let skyRequest = 0;
let skyLoadState = 'loading';
let skyErrorUntil = 0;
let localSkyId = 'preview-local-0';

// Scene definitions remain JSON; browser-local offsets are optional user preferences (stored by Rust).
function restoreLayout(target, scene) {
  try { const saved = savedLayout(scene); if (saved) target.ui_restore_layout(saved); }
  catch (error) { console.warn('Could not restore UI layout:', error); }
}
function saveLayout(target, scene) { storeLayout(scene, target.ui_layout()); }
function layoutScene() { return mode === 'menu' ? 'menu' : mode === 'gameplay' ? 'gameplay' : 'selector'; }
function resetPlayerInput() { gameplayMouse?.reset(); gameplayFire?.reset(); pendingLightPresentation = null; lastWeaponPresentation = null; if (gameplayLua) gameplayLua.emit('input.reset', {}); }
function releaseMouseLook() {
  dragLook = false;
  mouseCapture?.release();
}
function restoreWeaponOptic(runtime) {
  const optic = savedWeaponOptic();
  if (optic) runtime.dispatch('weapon.optic_set', optic);
}
let gameplayInputs = null, gameplayEvents = null;
const scriptEventChannels = new EventChannels(['engine.event']);
const tickEventChannels = new EventChannels(['weapon.shot', 'weapon.dry_fire', 'weapon.reload_started', 'projectile.spawned',
  'projectile.impact', 'projectile.expired', 'combat.damage', 'combat.death',
  'combat.near_miss', 'combat.suppression', 'engine.event']);
const outputChannels = new EventChannels(['player.controls_changed', 'camera.lens_changed', 'camera.zoom_lens_changed',
  'camera.controller_changed', 'image.correction_changed', 'first_person.frame', 'audio.layer', 'audio.play', 'lighting.flash_started', 'weapon.optic_changed', 'hud.pose_tracking', 'hud.map_request']);
const trailingOutputChannels = new EventChannels(['projectile.visuals', 'player.rig_changed', 'impact.visuals', 'hitbox.visuals']);
const outputConsumers = [
  event => saveSetting('player_controls', event), event => saveSetting('camera_lens', event),
  event => saveSetting('camera_zoom_lens', event), event => saveSetting('camera_controller', event),
  event => saveSetting('image_correction', event),
  () => {}, // First-person frames are handled as a batch below.
  event => sceneAudio.setLayer(event), event => sceneAudio.play(event),
  event => { if (Number.isSafeInteger(event.id) && event.id > 0) pendingLightPresentation = event.id; },
  event => saveSetting('weapon_optic', event, activeWeaponId),
  event => { inspectionPoseEnabled = event.enabled === true; lastInspectionPresentation = null; },
  event => { ensureMinimapRuntime()?.request(event); },
];
function ensureMinimapRuntime() {
  if (!active || !gameplayLua || !levelBase) return null;
  if (!minimapRuntime) {
    const game=active, lua=gameplayLua;
    try { minimapRuntime=new MinimapRuntime(new URL('./pkg/minimap-worker.js', import.meta.url).href,game,levelBase.href,()=>active===game && gameplayLua===lua,
      result=>lua.dispatch('hud.map_result',result),currentLevel?.rotate_z_180 !== false); }
    catch (error) { console.warn('Level map:',error); lua.dispatch('hud.map_result',{status:'unavailable'}); }
  }
  return minimapRuntime;
}
function publishFirstPersonPresentation(runtime) {
  const pose=firstPersonFrames?.presentation() ?? {visible:false};
  const signature=JSON.stringify(pose);
  // Gameplay aim is independent of whether the optional ammo HUD is visible.
  if (signature !== lastWeaponPresentation) {
    lastWeaponPresentation=signature;
    runtime.dispatch('first_person.presentation',pose);
  }
  if (inspectionPoseEnabled && signature !== lastInspectionPresentation) {
    lastInspectionPresentation=signature;
    runtime.dispatch('hud.weapon_presentation',pose);
  }
}
function reportWeaponFrame(frame) {
  if (frame) metrics.dataset.weapon = JSON.stringify({clip:frame.clip,index:frame.index,
    motion_x:frame.motion_x,motion_y:frame.motion_y,motion_roll:frame.motion_roll});
}
function consumeGameplayOutput(index, events) {
  if (index === 5) { reportWeaponFrame(firstPersonFrames?.requestBatch(events)); return; }
  for (const event of events) outputConsumers[index](event);
}
function consumeTrailingOutput(index, events) {
  // Impact, projectile and hitbox snapshots are validated and packed in Rust.
  if (index === 3) { active.applyHitboxVisuals(events); return; }
  if (index === 2) {
    const snapshot = events[events.length - 1];
    if (snapshot) metrics.dataset.impactEffects = JSON.stringify(active.applyImpactVisuals(snapshot));
    return;
  }
  if (index === 1) {
    for (const rig of events) savePlayerRig(currentLevel, rig);
    return;
  }
  const snapshot = events[events.length - 1];
  if (snapshot) active.applyProjectileVisuals(snapshot);
}
function dispatchGameplayEvents(runtime) {
  runtime.dispatchPending(gameplayEvents);
}
function closeScripts() {
  active?.clearHitboxVisuals?.();
  inputLatency?.clearPending();
  gameplayLua?.close(); menuLua?.close();
  gameplayLua = null; menuLua = null; lastScriptTime = null; pendingLightPresentation = null; activeWeaponId = null;
  inspectionPoseEnabled = false; lastInspectionPresentation = null; lastWeaponPresentation = null; gameplayEntityIds = null;
}
function sceneAsset(scene, base, id) {
  const asset = scene.resources?.assets?.[id];
  if (!asset || asset.kind !== 'scene') throw new Error(`Unknown scene asset ${id}.`);
  const url = new URL(asset.uri, base);
  if (url.origin !== location.origin) throw new Error('Scene asset must be local to this preview.');
  return url.href;
}
// The ECS pause menu requests changes; the host validates device limits and
// persists preferences, then sends the accepted values back to the scene.
function menuSettings(error = '') {
  if (!active?.menu_settings) return {};
  return JSON.parse(active.menu_settings(displayPanel, displayViewport.width, displayViewport.height,
    currentLevel?.name, currentLevel?.format, error));
}
function processMenuSettings(runtime) {
  for (const request of runtime.drain('game.mode_request')) {
    if (request?.mode==='pursuit' && (request.action==='start' || request.action==='reset') && gameplayLua)
      gameplayLua.dispatch('game.mode_request',request);
    runtime.dispatch('menu.settings',menuSettings());
  }
  for (const request of runtime.drain('settings.aspect_request')) {
    const error=displayPanel.set_aspect(String(request.aspect),displayViewport.width,displayViewport.height,active.max_texture_dimension());
    runtime.dispatch('menu.settings', menuSettings(error));
  }
  for (const request of runtime.drain('settings.display_request')) {
    const error = displayPanel.set(JSON.stringify(request ?? null), active.max_texture_dimension());
    runtime.dispatch('menu.settings', menuSettings(error));
  }
  for (const request of runtime.drain('settings.hud_request')) {
    if (displayPanel.set_show_performance(JSON.stringify(request?.show_performance ?? null))) {
      gameplayLua?.dispatch('hud.preferences_changed', {show_performance:displayPanel.show_performance()});
    }
    runtime.dispatch('menu.settings', menuSettings());
  }
  const allowed={controls_speed:'number_changed',controls_sensitivity:'number_changed',controls_invert:'bool_changed',
    level_draw_distance:'number_changed',level_auto:'bool_changed',level_lod:'number_changed',
    sky_enabled:'bool_changed',audio_gain:'number_changed',audio_muted:'bool_changed'};
  for (const request of runtime.drain('settings.gameplay_request')) {
    const kind=allowed[request.target];
    const valid=kind && request.kind===kind && (kind==='number_changed' ?
      typeof request.value==='number' && Number.isFinite(request.value) : typeof request.value==='boolean');
    if (valid && gameplayLua) {
      if (request.target==='sky_enabled') gameplayLua.dispatch('settings.sky_request',{enabled:request.value});
      else if (request.target==='audio_gain') gameplayLua.dispatch('settings.sound_request',{gain:request.value});
      else if (request.target==='audio_muted') gameplayLua.dispatch('settings.sound_request',{muted:request.value});
      else if (gameplayEntityIds?.[request.target]) gameplayLua.dispatch('ui.action',
        {id:gameplayEntityIds[request.target],kind:{type:kind,value:request.value}});
      for (const event of gameplayLua.drain('player.controls_changed')) saveSetting('player_controls', event);
      for (const event of gameplayLua.drain('audio.layer')) sceneAudio.setLayer(event);
    }
    runtime.dispatch('menu.settings',menuSettings());
  }
}
function processSceneRequests(runtime) {
  for (const request of runtime.drain('engine.scene')) {
    if (mode === 'gameplay' && request.action === 'push' && sceneAsset(gameplayScene, gameplayUrl, request.scene) === menuUrl.href) {
      gameplayMouse?.reset(); gameplayFire?.reset(); pendingLightPresentation = null;
      // Keep the ammo callout's pose subscription across pause/resume.
      lastInspectionPresentation = null; lastWeaponPresentation = null;
      runtime.dispatch('input.reset', {});
      voxelGame?.voxel_suspend(); inputLatency?.clearPending(); sceneAudio?.stopVoices();
      releaseMouseLook();
      active.ui_focus(false); active.ui_pointer_gone();
      active.set_menu_visible(true); active.set_paused(true); mode = 'menu';
      menuLua?.dispatch('menu.opened', {});
      menuLua?.dispatch('menu.settings', menuSettings());
      restoreLayout(active, 'menu');
      if (document.activeElement === canvas) active.ui_focus(true);
      status.textContent = 'Paused';
      lastScriptTime = null;
      return;
    }
    if (mode === 'menu' && request.action === 'pop') {
      active.ui_focus(false); active.ui_pointer_gone();
      active.set_menu_visible(false); active.set_paused(document.hidden); mode = 'gameplay';
      if (document.activeElement === canvas) active.ui_focus(true);
      status.textContent = 'Live WebGPU canvas';
      lastScriptTime = null;
      return;
    }
    if ((mode === 'gameplay' || mode === 'menu') && request.action === 'replace') {
      const scene = mode === 'menu' ? menuScene : gameplayScene;
      const base = mode === 'menu' ? menuUrl : gameplayUrl;
      if (sceneAsset(scene, base, request.scene) === selectorUrl.href) { void returnToSelector(); return; }
    }
    throw new Error('Unsupported scene request from Lua.');
  }
}

function showError(error) {
  stopped = true;
  status.textContent = 'Renderer unavailable';
  message.hidden = false;
  message.textContent = error instanceof Error ? error.message : String(error);
  console.error('CounterSplat canvas:', error);
}
async function getBytes(path, signal, optional = false) {
  const response = await fetch(path, {cache: 'no-store', signal});
  if (optional && response.status === 404) return new Uint8Array();
  if (!response.ok) throw new Error(`Could not load ${path} (HTTP ${response.status}).`);
  return new Uint8Array(await response.arrayBuffer());
}
function cancelLoading() {
  if (mode !== 'loading' || !active || !loadingAbort) return;
  ++generation;
  loadingAbort.abort(); loadingAbort = null;
  active.finish_loading('cancelled'); active.set_busy(false);
  active.set_notice('Loading cancelled.');
  mode = 'selector'; status.textContent = 'Choose a level';
}
function nextFrame() { return new Promise(resolve => requestAnimationFrame(resolve)); }
function requestWorker(worker, payload, signal, transfer = []) {
  return new Promise((resolve, reject) => {
    const cleanup = () => { worker.removeEventListener('message', onMessage); worker.removeEventListener('error', onError); signal.removeEventListener('abort', onAbort); };
    const onMessage = ({data}) => { cleanup(); data.ok ? resolve(data) : reject(new Error(data.error)); };
    const onError = event => { cleanup(); reject(new Error(event.message || 'Asset worker failed.')); };
    const onAbort = () => { cleanup(); reject(new DOMException('Loading cancelled', 'AbortError')); };
    if (signal.aborted) { onAbort(); return; }
    worker.addEventListener('message', onMessage); worker.addEventListener('error', onError); signal.addEventListener('abort', onAbort, {once:true});
    try { worker.postMessage(payload, transfer); } catch (error) { cleanup(); reject(error); }
  });
}
async function preloadSceneAssets(scene, base, cache, signal, tracker, from, to) {
  const entries = Object.entries(scene.resources?.assets || {}).filter(([, asset]) => loadPolicy(asset.load) === 'preload' && asset.kind !== 'sog_level');
  let done = 0;
  tracker.update('Loading scene assets', from, to, 0, Math.max(entries.length, 1));
  await mapLimit(entries, 6, async ([id, asset]) => {
    const url = new URL(asset.uri, base);
    if (url.origin !== location.origin) throw new Error(`Scene asset ${id} must be local to this preview.`);
    if (!cache.has(url.href)) {
      const bytes = await getBytes(url, signal);
      cache.set(url.href, bytes);
      const size = [...cache.values()].reduce((sum, value) => sum + value.byteLength, 0);
      if (size > MAX_PRELOAD_BYTES) throw new Error('Scene preload exceeds the 512 MiB browser limit. Mark large assets streamable.');
    }
    tracker.update('Loading scene assets', from, to, ++done, Math.max(entries.length, 1));
  });
  if (!entries.length) tracker.update('Loading scene assets', from, to, 1, 1);
}
async function preloadScripts(gameScene, gameBase, menu, menuBase, signal, tracker, cache) {
  const gamePlan = planScripts(gameScene, new URL(gameBase), location.origin);
  const menuPlan = planScripts(menu, new URL(menuBase), location.origin);
  const tasks = [...gamePlan.map(item => ({...item, scope:'game'})), ...menuPlan.map(item => ({...item, scope:'menu'})),
    {id:'engine-api', url:new URL('./pkg/engine_api.lua', import.meta.url), scope:'engine', execute:true}];
  let done = 0;
  tracker.update('Loading Lua scripts', 0.65, 0.75, 0, tasks.length);
  const decoder = new TextDecoder('utf-8', {fatal:true});
  const loaded = await mapLimit(tasks, 6, async item => {
    const bytes = await getBytes(item.url, signal);
    if (!bytes.length || bytes.length > 1024 * 1024) throw new Error(`Script ${item.id} has an invalid size.`);
    cache.set(item.url.href, bytes);
    const size = [...cache.values()].reduce((sum, value) => sum + value.byteLength, 0);
    if (size > MAX_PRELOAD_BYTES) throw new Error('Scene preload exceeds the 512 MiB browser limit. Mark large assets streamable.');
    const source = decoder.decode(bytes);
    tracker.update('Loading Lua scripts', 0.65, 0.75, ++done, tasks.length);
    return {...item, source};
  });
  return {
    gameplay: loaded.filter(item => item.scope === 'game' && item.execute).map(item => item.source),
    menu: loaded.filter(item => item.scope === 'menu' && item.execute).map(item => item.source),
    engine: loaded.find(item => item.scope === 'engine').source,
  };
}
async function preloadSkies(scene, base, cache, signal, tracker) {
  const skies = Object.entries(scene.resources?.assets || {}).filter(([, asset]) => asset.kind === 'radiance_hdr' && loadPolicy(asset.load) === 'preload');
  if (skies.length > 8) throw new Error('The preview supports at most eight skybox assets.');
  tracker.update('Decoding preloaded skies', 0.75, 0.85, 0, Math.max(skies.length, 1));
  if (!skies.length) { tracker.update('Decoding preloaded skies', 0.75, 0.85, 1, 1); return []; }
  const worker = new Worker(new URL('./pkg/skybox-worker.js', import.meta.url), {type:'module'});
  try {
    const prepared = [];
    for (const [id, asset] of skies) {
      const url = new URL(asset.uri, base);
      const bytes = cache.get(url.href);
      if (!bytes) throw new Error(`Preloaded sky ${id} is missing.`);
      const data = await requestWorker(worker, {request:0, asset:id, select:false, bytes:bytes.slice()}, signal);
      prepared.push({id, size:data.size, pixels:data.pixels});
      tracker.update('Decoding preloaded skies', 0.75, 0.85, prepared.length, skies.length);
    }
    return prepared;
  } finally { worker.terminate(); }
}
function parseJson(bytes) { return JSON.parse(new TextDecoder().decode(bytes)); }
function showSkyError(error) {
  skyLoadState = 'error';
  skyErrorUntil = performance.now() + 5000;
  message.hidden = false;
  message.textContent = `Skybox: ${error instanceof Error ? error.message : String(error)}`;
  console.warn('CounterSplat skybox:', error);
}
function startSkyWorker(game, token, preloaded) {
  const assets = gameplayScene.resources?.assets || {};
  const skyIds = new Set(gameplayScene.entities
    .filter(entity => entity.layer === 'scene' && entity.components?.Skybox)
    .map(entity => entity.components.Skybox.asset ?? 'default'));
  if (skyIds.size > 8) throw new Error('The preview supports at most eight skybox assets.');
  const skies = [...skyIds].map(id => {
    const asset = assets[id];
    if (!asset || asset.kind !== 'radiance_hdr') throw new Error(`Scene skybox asset ${id} must be a radiance_hdr asset.`);
    return {id, url: new URL(asset.uri, gameplayUrl), load: loadPolicy(asset.load)};
  });
  let suffix = 0;
  while (Object.hasOwn(assets, `preview-local-${suffix}`)) suffix++;
  localSkyId = `preview-local-${suffix}`;
  skyRequest = 0;
  skyLoadState = skies.some(sky => preloaded.has(sky.id)) ? 'ready' : 'loading';
  skyDecoder = new Worker(new URL('./pkg/skybox-worker.js', import.meta.url), {type: 'module'});
  skyDecoder.onmessage = ({data}) => {
    if (generation !== token || active !== game || (mode !== 'gameplay' && mode !== 'menu')) return;
    if (data.select && data.request !== skyRequest) return;
    if (!data.ok) { showSkyError(data.error); return; }
    try {
      game.install_skybox(data.asset, data.size, data.pixels);
      if (data.select) game.select_skybox(data.asset);
      skyLoadState = 'ready';
    } catch (error) { showSkyError(error); }
  };
  skyDecoder.onerror = event => { if (generation === token) showSkyError(new Error(`Sky worker failed: ${event.message}`)); };
  for (const {id, url, load} of skies) {
    if (load === 'preload') {
      if (!preloaded.has(id)) throw new Error(`Preloaded sky ${id} was not prepared.`);
      continue;
    }
    getBytes(url).then(bytes => {
      if (generation === token && skyDecoder) skyDecoder.postMessage({request: 0, asset: id, select: false, bytes}, [bytes.buffer]);
    }).catch(error => { if (generation === token) showSkyError(error); });
  }
}
function stopWorkers() {
  sceneAudio?.clear();
  firstPersonFrames?.close(); firstPersonFrames = null;
  voxelGame?.voxel_stop(); voxelGame = null;
  minimapRuntime?.stop(); minimapRuntime = null;
  if (skyDecoder) skyDecoder.terminate();
  skyDecoder = null;
}

// Convert an unstreamed SOG into one synthetic streaming leaf without modifying its files.
function singleSogManifest(meta) {
  const count = meta?.count;
  const mins = meta?.means?.mins;
  const maxs = meta?.means?.maxs;
  if (!Number.isInteger(count) || count <= 0 || count > 10_000_000 ||
      !Array.isArray(mins) || !Array.isArray(maxs) || mins.length !== 3 || maxs.length !== 3) {
    throw new Error('The single-SOG metadata is invalid or too large for this preview.');
  }
  const unlog = n => Math.sign(n) * Math.expm1(Math.abs(n));
  const min = mins.map(unlog), max = maxs.map(unlog);
  if (![...min, ...max].every(Number.isFinite) || min.some((n, i) => n > max[i])) {
    throw new Error('The single-SOG bounds are invalid.');
  }
  return {counts: [count], lodLevels: 1, filenames: ['single/meta.json'],
    tree: {bound: {min, max}, lods: {'0': {file: 0, offset: 0, count}}}};
}
async function mapLimit(values, limit, task) {
  const results = new Array(values.length);
  let next = 0;
  async function worker() {
    while (next < values.length) {
      const index = next++;
      results[index] = await task(values[index], index);
    }
  }
  await Promise.all(Array.from({length: Math.min(limit, values.length)}, worker));
  return results;
}
// Decode catalog previews once. Both selector and paused gameplay use the
// same bounded local image cache and canvas texture upload.
async function loadLevelPreview(level) {
  if (!/^[0-9a-f]{8}$/.test(level.id) || level.thumbnail !== `assets/levels/${level.id}/preview.webp`) return null;
  if (previewImages.has(level.id)) return previewImages.get(level.id);
  if (previewPending.has(level.id)) return previewPending.get(level.id);
  const task=(async()=>{
    const bytes=await getBytes(new URL(level.thumbnail, import.meta.url),undefined,true);
    if (!bytes.length || bytes.length>4*1024*1024) return null;
    const bitmap=await createImageBitmap(new Blob([bytes],{type:'image/webp'}),
      {resizeWidth:320,resizeHeight:320,resizeQuality:'high'});
    try {
      const buffer=document.createElement('canvas');buffer.width=bitmap.width;buffer.height=bitmap.height;
      const context=buffer.getContext('2d',{willReadFrequently:true});
      if (!context || !buffer.width || !buffer.height || buffer.width>1024 || buffer.height>1024) return null;
      context.drawImage(bitmap,0,0);
      const image={width:buffer.width,height:buffer.height,rgba:new Uint8Array(context.getImageData(0,0,buffer.width,buffer.height).data)};
      previewImages.set(level.id,image);return image;
    } finally { bitmap.close(); }
  })();
  previewPending.set(level.id,task);
  try { return await task; } finally { previewPending.delete(level.id); }
}
async function populateSelectorPreviews(selector) {
  await mapLimit(catalog.levels,4,async level=>{
    try {
      const image=await loadLevelPreview(level);
      if (image && active===selector) selector.install_level_preview(level.id,image.width,image.height,image.rgba);
    } catch (error) { console.warn(`Preview unavailable for ${level.id}:`,error); }
  });
}
async function launchLevel(id, sceneId = '') {
  if (mode !== 'selector' || !active) return;
  const selected = catalog.levels.find(level => level.id === id);
  if (!selected) { active.set_notice('That level is no longer in the catalog.'); return; }
  const selector = active;
  const controller = new AbortController(), signal = controller.signal;
  const token = ++generation;
  let pendingGame = null;
  loadingAbort = controller;
  selectedId = id;
  mode = 'loading';
  selector.begin_loading(selected.name);
  status.textContent = 'Loading level';
  const tracker = new LoadingProgress((phase, progress, completed, total) => {
    if (generation === token && active === selector) selector.update_loading(phase, progress, completed, total);
  });
  try {
    // Give the canvas a frame to display its ECS loading overlay before starting work.
    await nextFrame(); await nextFrame();
    if (generation !== token) return;
    const chosenUrl = sceneId ? localSceneUrl(sceneId, new URL('.', import.meta.url).href) : defaultGameplayUrl;
    const chosenBytes = sceneId ? await getBytes(chosenUrl, signal) : defaultGameplayBytes;
    const chosenScene = parseJson(chosenBytes);
    const chosenMenuUrl = new URL(sceneAsset(chosenScene, chosenUrl, 'pause_menu'));
    const policies = levelPolicies(selected.assets);
    const decoderCount = chunk_decoder_count(navigator.hardwareConcurrency, chosenScene.resources?.preview?.chunk_decoder_workers);
    const decoderUrl = new URL('./decoder.js', import.meta.url).href;
    if (loadPolicy(chosenScene.resources?.assets?.pause_menu?.load) !== 'preload')
      throw new Error('The gameplay pause menu must be preloaded by this host.');
    levelBase = new URL(`./assets/levels/${encodeURIComponent(id)}/`, import.meta.url);
    tracker.update('Reading level manifest', 0, 0.1, 0, 4);
    let manifestTasks = 0;
    const fetchManifest = async (path, optional = false) => {
      const bytes = await getBytes(path, signal, optional);
      tracker.update('Reading level manifest', 0, 0.1, ++manifestTasks, 4);
      return bytes;
    };
    const [primary, settings, inventoryBytes, sourceBytes] = await Promise.all([
      fetchManifest(new URL(selected.format === 'single_sog' ? 'meta.json' : 'lod-meta.json', levelBase)),
      fetchManifest(new URL('viewer-settings.json', levelBase), true),
      fetchManifest(new URL('download-inventory.json', levelBase), true),
      fetchManifest(new URL('source.json', levelBase), true),
    ]);
    let lodBytes = primary, filenames, lodCount, singleMetadata;
    let capacity = chosenScene.resources?.preview?.capacity;
    if (!Number.isInteger(capacity) || capacity <= 0) throw new Error('Gameplay scene capacity is invalid.');
    if (selected.format === 'single_sog') {
      const synthetic = singleSogManifest(parseJson(primary));
      lodBytes = new TextEncoder().encode(JSON.stringify(synthetic));
      singleMetadata = primary.slice(); filenames = synthetic.filenames; lodCount = synthetic.lodLevels;
      capacity = Math.max(capacity, Math.ceil(synthetic.counts[0] * 100 / 85) + 1);
    } else if (selected.format === 'streamed_sog') {
      const lod = parseJson(primary);
      if (!Array.isArray(lod.filenames) || !lod.filenames.length || lod.filenames.length > 10_000)
        throw new Error('The level does not contain a valid chunk list.');
      filenames = lod.filenames; lodCount = lod.lodLevels;
      // A fixed pool starves a dense scan: the scene's `capacity` is only the floor. Size the pool to about 70% of
      // the level's finest-detail splats, up to `max_capacity`, which bounds GPU memory (about 49 bytes a splat).
      const finest = Array.isArray(lod.counts) ? Number(lod.counts[0]) : 0;
      const ceiling = chosenScene.resources?.preview?.max_capacity;
      if (Number.isInteger(ceiling) && ceiling > capacity && Number.isFinite(finest) && finest > 0)
        capacity = Math.max(capacity, Math.min(ceiling, Math.ceil(finest * 0.7)));
    } else throw new Error('This level format is not supported.');
    tracker.update('Reading chunk metadata', 0.1, 0.5, 0, filenames.length);
    const {metas, levelAssets} = await loadLevelMetadata(
      new URL('./pkg/metadata-worker.js', import.meta.url).href,
      {level:selected, lodCount, filenames, base:levelBase.href, singleMetadata, inventoryBytes,
        viewerSettings:settings.length > 0, hasSource:sourceBytes.length > 0,
        source:chosenScene.resources?.preview?.level_asset || 'level',
        concurrency:chosenScene.resources?.preview?.metadata_fetch_concurrency ?? 12}, signal,
      (done, total) => tracker.update('Reading chunk metadata', 0.1, 0.5, done, total));
    if (generation !== token) return;

    const sceneAssets = new Map([[selectorUrl.href, selectorBytes], [chosenUrl.href, chosenBytes]]);
    await preloadSceneAssets(chosenScene, chosenUrl, sceneAssets, signal, tracker, 0.5, 0.6);
    const chosenMenuBytes = sceneAssets.get(chosenMenuUrl.href);
    if (!chosenMenuBytes) throw new Error('The preloaded pause menu is missing.');
    const chosenMenuScene = parseJson(chosenMenuBytes);
    await preloadSceneAssets(chosenMenuScene, chosenMenuUrl, sceneAssets, signal, tracker, 0.6, 0.65);
    const scripts = await preloadScripts(chosenScene, chosenUrl, chosenMenuScene, chosenMenuUrl, signal, tracker, sceneAssets);
    const skies = await preloadSkies(chosenScene, chosenUrl, sceneAssets, signal, tracker);
    tracker.update('Decoding splat chunks', 0.85, 0.96, 0, 1);
    const chunks = policies.splat_chunks === 'preload'
      ? await preload_level_chunks(levelBase.href, decoderUrl, metas, filenames, selected.format === 'single_sog', decoderCount,
          MAX_PRELOAD_BYTES, (done, total) => tracker.update('Decoding splat chunks', 0.85, 0.96, done, total), signal)
      : (tracker.update('Splat chunks will stream in the scene', 0.85, 0.96, 1, 1), null);
    if (generation !== token) return;
    tracker.update('Starting renderer', 0.96, 0.99, 0, 1);
    selector.set_loading_cancellable(false);
    await nextFrame();
    if (generation !== token) return;

    // All configured preloads are ready. Keep the last loading frame until the new scene is ready.
    selector.finish_loading('complete');
    loadingAbort = null;
    mode = 'starting';
    active = null;
    selector.free();
    pendingGame = await Viewer.create(canvas, lodBytes, metas, settings, [chosenBytes, chosenMenuBytes], capacity, selected.rotate_z_180 !== false, policies.splat_chunks === 'preload', JSON.stringify(levelAssets), stackRequested);
    if (generation !== token) { pendingGame.free(); pendingGame = null; return; }
    for (const sky of skies) pendingGame.install_skybox(sky.id, sky.size, sky.pixels);
    if (chunks) pendingGame.insert_preloaded_chunks(chunks);
    pendingGame.restoreSavedSettings();
    pendingGame.publishPlayerRigData(selected);
    pendingGame.ecs_operation('gameplay', JSON.stringify({op:'set_resource', name:'hud.preferences',
      value:{show_performance:displayPanel.show_performance()}}));
    // Publish selected level identity as data for scene-owned systems.
    pendingGame.ecs_operation('gameplay', JSON.stringify({op:'set_resource', name:'scene.level',
      value:{id:selected.id, name:selected.name}}));
    gameplayScripts = scripts.gameplay; menuScripts = scripts.menu; engineApi = scripts.engine;
    if (frameProfiler) { frameProfiler.reset(); inputLatency.reset(); profileState = null; }
    gameplayLua = await LuaScene.create(luaFactory, pendingGame, 'gameplay', engineApi, gameplayScripts, !!frameProfiler);
    gameplayEntityIds = JSON.parse(pendingGame.ecs_operation('gameplay', JSON.stringify({op:'get_resource', name:'scene.entities'}))).value ?? null;
    activeWeaponId = gameplayEntityIds?.first_person ?? null;
    restoreWeaponOptic(gameplayLua);
    menuLua = await LuaScene.create(luaFactory, pendingGame, 'menu', engineApi, menuScripts);
    if (generation !== token) { closeScripts(); pendingGame.free(); pendingGame = null; return; }
    active = pendingGame;
    pendingGame = null;
    gameplayUrl = chosenUrl; gameplayBytes = chosenBytes; gameplayScene = chosenScene;
    menuUrl = chosenMenuUrl; menuBytes = chosenMenuBytes; menuScene = chosenMenuScene;
    restoreLayout(active, 'gameplay');
    active.ui_set_arrange_mode(arrangeMode);
    currentLevel = selected;
    const previewOwner=active;
    void loadLevelPreview(selected).then(image=>{
      if (image && active===previewOwner) previewOwner.install_menu_preview(image.width,image.height,image.rgba);
    }).catch(error=>console.warn(`Preview unavailable for ${selected.id}:`,error));
    preloadedSceneAssets = sceneAssets;
    await sceneAudio.load(chosenScene, String(chosenUrl), sceneAssets);
    if (generation !== token) return;
    const footage = chosenScene.resources?.assets?.['first-person-footage'];
    if (footage) {
      if (footage.kind !== 'first_person_manifest' || loadPolicy(footage.load) !== 'preload') throw new Error('First-person footage manifest must be a preloaded scene asset.');
      const bytes = sceneAssets.get(new URL(footage.uri, chosenUrl).href);
      if (!bytes) throw new Error('First-person footage manifest is missing.');
      firstPersonFrames = new FirstPersonFrames(chosenScene, String(chosenUrl), parseJson(bytes), sceneAssets, active);
    }
    mode = 'gameplay';
    const game = active;
    // Chunk decoding and streaming run in Rust; the frame loop drives them.
    game.start_chunk_streaming(levelBase.href, decoderUrl, selected.format === 'single_sog', decoderCount);
    startSkyWorker(game, token, new Set(skies.map(sky => sky.id)));
    game.start_voxels(new URL('./pkg/voxel-worker.js', import.meta.url).href, levelBase.href); voxelGame = game;
    // Prewarm only scans without voxel files: their fallback decodes SOG means.
    if (selected.collision === false) ensureMinimapRuntime();
    if (frameProfiler) game.voxel_enable_latency();
    status.textContent = 'Streaming scan';
    message.hidden = true;
  } catch (error) {
    if (generation !== token) return;
    loadingAbort = null;
    controller.abort();
    stopWorkers(); closeScripts(); preloadedSceneAssets.clear();
    if (pendingGame) { pendingGame.free(); pendingGame = null; }
    console.error('Level launch:', error);
    if (active && active !== selector) { active.free(); active = null; }
    if (!active) {
      try { active = await Selector.create(canvas, selectorBytes, catalogBytes, sceneCatalogBytes, selectedId, selectedSceneId); }
      catch (restoreError) { showError(restoreError); return; }
    } else {
      active.finish_loading('failed');
    }
    mode = 'selector';
    restoreLayout(active, 'selector');
    active.ui_set_arrange_mode(arrangeMode);
    active.set_busy(false);
    active.set_notice(`Could not load scene: ${error instanceof Error ? error.message : String(error)}`);
    void populateSelectorPreviews(active);
    status.textContent = 'Choose a level';
  }
}
async function returnToSelector() {
  if ((mode !== 'gameplay' && mode !== 'menu') || !active) return;
  ++generation;
  const game = active;
  releaseMouseLook();
  active = null;
  mode = 'starting';
  stopWorkers();
  closeScripts();
  preloadedSceneAssets.clear();
  game.free();
  message.hidden = true;
  try {
    sceneCatalogBytes = await getBytes(sceneCatalogUrl);
    active = await Selector.create(canvas, selectorBytes, catalogBytes, sceneCatalogBytes, selectedId, selectedSceneId);
    restoreLayout(active, 'selector');
    active.ui_set_arrange_mode(arrangeMode);
    mode = 'selector';
    void populateSelectorPreviews(active);
    status.textContent = 'Choose a level';
  } catch (error) { showError(error); }
}

// Input uses canvas-local CSS points and always targets the active scene.
function attachInput() {
  arrangeButton.addEventListener('click', () => {
    arrangeMode = !arrangeMode;
    if (arrangeMode) { releaseMouseLook(); resetPlayerInput(); }
    arrangeButton.setAttribute('aria-pressed', String(arrangeMode));
    arrangeButton.textContent = arrangeMode ? 'Done arranging' : 'Arrange UI';
    arrangeButton.title = arrangeMode ? 'Finish arranging UI elements' : 'Move individual UI elements';
    resetLayoutButton.hidden = !arrangeMode;
    if (active) active.ui_set_arrange_mode(arrangeMode);
  });
  resetLayoutButton.addEventListener('click', () => {
    if (!active) return;
    active.ui_reset_layout();
    saveLayout(active, layoutScene());
  });
  const modifiers = event => (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) |
    (event.shiftKey ? 4 : 0) | (event.metaKey ? 8 : 0);
  const point = event => { const bounds = canvas.getBoundingClientRect(); return [event.clientX - bounds.left, event.clientY - bounds.top]; };
  const button = event => ({0: 0, 1: 2, 2: 1})[event.button];
  const send = action => { if (active) { try { action(active); } catch (error) { console.warn('UI input:', error); } } };
  const gameplayPointer = (scene, event) => {
    if (mode !== 'gameplay' || arrangeMode || document.hidden) return false;
    if (document.pointerLockElement === canvas || dragLook) return true;
    const [x,y] = point(event);
    return !scene.ui_hit_test(x,y);
  };
  canvas.addEventListener('focus', () => send(scene => scene.ui_focus(true)));
  canvas.addEventListener('blur', () => { resetPlayerInput(); send(scene => { scene.ui_focus(false); scene.ui_pointer_gone(); }); });
  canvas.addEventListener('pointermove', event => {
    const startingRight = event.pointerType === 'mouse' && event.button === 2 && (event.buttons & 2) !== 0;
    send(scene => {
      gameplayMouse.update(event, startingRight && gameplayPointer(scene, event));
      const startingLeft = event.pointerType === 'mouse' && event.button === 0 && (event.buttons & 1) !== 0;
      gameplayFire.update(event, startingLeft && gameplayPointer(scene, event));
    });
    if (mode === 'gameplay' && (document.pointerLockElement === canvas || dragLook)) {
      if (gameplayLua && Number.isFinite(event.movementX) && Number.isFinite(event.movementY)) {
        gameplayLua.emit('input.mouse_delta', {dx:event.movementX, dy:event.movementY});
        inputLatency?.recordInput();
      }
      return;
    }
    send(scene => { const [x,y] = point(event); scene.ui_pointer_move(x,y); });
  });
  canvas.addEventListener('pointerdown', event => send(scene => {
    const which = button(event); if (which === undefined) return;
    event.preventDefault(); canvas.focus({preventScroll:true});
    const [x,y] = point(event);
    const gameplay = gameplayPointer(scene, event);
    gameplayFire.update(event, gameplay);
    if (gameplayMouse.update(event, gameplay)) {
      if (document.pointerLockElement !== canvas) { canvas.setPointerCapture(event.pointerId); scene.ui_pointer_gone(); }
      return;
    }
    const world = gameplay && scene.player_mouse_look_enabled();
    if (world && which === 0 && event.pointerType === 'mouse' && document.pointerLockElement !== canvas) {
      // Drag-to-look remains available when an embedded browser does not support pointer lock.
      dragLook = true; canvas.setPointerCapture(event.pointerId); scene.ui_pointer_gone();
      mouseCapture.request();
      return;
    }
    if (document.pointerLockElement === canvas) return;
    canvas.setPointerCapture(event.pointerId);
    scene.ui_pointer_move(x,y); scene.ui_pointer_button(x,y,which,true,modifiers(event));
  }));
  canvas.addEventListener('pointerup', event => send(scene => {
    const which = button(event); if (which === undefined) return;
    const zoomWasDown = gameplayMouse.rightDown;
    gameplayMouse.update(event, false);
    gameplayFire.update(event, false);
    if (zoomWasDown && which === 1) return;
    if (dragLook) { dragLook = false; return; }
    if (document.pointerLockElement === canvas) return;
    const [x,y] = point(event); scene.ui_pointer_button(x,y,which,false,modifiers(event));
    if (event.pointerType !== 'mouse') scene.ui_pointer_gone();
  }));
  canvas.addEventListener('pointercancel', event => send(scene => {
    resetPlayerInput();
    dragLook = false;
    const [x,y] = point(event);
    for (const which of [0,1,2]) scene.ui_pointer_button(x,y,which,false,modifiers(event));
    scene.ui_pointer_gone();
  }));
  canvas.addEventListener('pointerleave', event => { if (!event.buttons) send(scene => scene.ui_pointer_gone()); });
  canvas.addEventListener('wheel', event => {
    if (!active || document.pointerLockElement === canvas || dragLook) return;
    const [x,y] = point(event);
    // Hovering an ECS panel is enough to scroll; egui only reports pointer
    // capture while a button is held. Line/page wheel units need normalization.
    const hoveredUi = mode === 'gameplay' && active.ui_hit_test(x,y);
    if (hoveredUi || active.ui_wants_pointer()) {
      event.preventDefault();
      const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? canvas.getBoundingClientRect().height : 1;
      send(scene => { scene.ui_pointer_move(x,y); scene.ui_scroll(-event.deltaX*scale,-event.deltaY*scale); });
    }
  }, {passive:false});
  canvas.addEventListener('contextmenu', event => { if (mode === 'gameplay' || active?.ui_wants_pointer()) event.preventDefault(); });
  const key = event => {
    if (!active) return;
    const wantsKeyboard = active.ui_wants_keyboard();
    if (!wantsKeyboard && event.key !== 'Tab') return;
    send(scene => scene.ui_key(event.key,event.type === 'keydown',event.repeat,modifiers(event)));
    if (event.type === 'keydown' && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && !event.isComposing)
      send(scene => scene.ui_text(event.key));
    if (event.key === 'Tab' || wantsKeyboard) event.preventDefault();
  };
  canvas.addEventListener('keydown', key);
  canvas.addEventListener('keyup', key);
  // Forward physical key events to the current ECS world. Lua decides what they mean.
  const gameKey = event => {
    if (mode === 'gameplay' && event.code === 'Backquote' && event.type === 'keydown' && !event.repeat &&
        document.activeElement === canvas && !active.ui_wants_keyboard() && !arrangeMode) {
      // The Lua overlay receives the same key; releasing capture keeps its controls reachable.
      releaseMouseLook();
      active.ui_pointer_gone();
    }
    const script = mode === 'gameplay' ? gameplayLua : mode === 'menu' ? menuLua : null;
    if (!script || event.isComposing || event.key.length > 128) return;
    const gameplay = mode === 'gameplay' && document.activeElement === canvas && !active.ui_wants_keyboard() &&
      !arrangeMode && !document.hidden && !event.metaKey && !event.altKey;
    script.emit('input.key', {key: event.key, code: event.code, pressed: event.type === 'keydown', is_repeat: event.repeat, gameplay});
    if (event.key === 'Escape') { if (event.type === 'keydown') lastEscapeTime = performance.now(); event.preventDefault(); }
    else if (gameplay) event.preventDefault();
  };
  window.addEventListener('keydown', gameKey);
  window.addEventListener('keyup', gameKey);
  window.addEventListener('blur', resetPlayerInput);
  document.addEventListener('pointerlockerror', () => mouseCapture.error());
  document.addEventListener('pointerlockchange', () => {
    const change = mouseCapture.changed(mode === 'gameplay' && !arrangeMode);
    if (change === 'locked') { dragLook = false; return; }
    if (change !== 'released' && change !== 'unexpected') return;
    resetPlayerInput();
    if (change === 'released') return;
    // Some browsers consume Escape while releasing pointer lock; preserve the menu shortcut.
    const lostAt = performance.now();
    setTimeout(() => {
      if (mode === 'gameplay' && !document.hidden && document.hasFocus() && lastEscapeTime < lostAt - 100)
        gameplayLua?.emit('input.key', {key:'Escape',code:'Escape',pressed:true,is_repeat:false,gameplay:false});
    }, 60);
  });
  canvas.addEventListener('compositionend', event => { if (event.data && active?.ui_wants_keyboard()) send(scene => scene.ui_text(event.data)); });
  canvas.addEventListener('dragover', event => { if (mode === 'gameplay') event.preventDefault(); });
  canvas.addEventListener('drop', async event => {
    if (mode !== 'gameplay') return;
    event.preventDefault();
    const file = event.dataTransfer.files[0];
    if (!file) return;
    if (!file.name.toLowerCase().endsWith('.hdr') || file.size > 64 * 1024 * 1024) { showSkyError('Choose a Radiance .hdr file up to 64 MiB.'); return; }
    const request = ++skyRequest, token = generation;
    skyLoadState = 'loading';
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      if (token === generation && request === skyRequest && skyDecoder) skyDecoder.postMessage({request, asset:localSkyId, select:true, bytes}, [bytes.buffer]);
    } catch (error) { if (token === generation && request === skyRequest) showSkyError(error); }
  });
}
function tick(timestamp) {
  if (stopped) return;
  // Collision replies that arrived since the last frame apply before input and scripts see the world.
  voxelGame?.voxel_receive();
  const stackFrame = stackCapture?.beginFrame(mode === 'gameplay' && !!active, gameplayLua);
  const profileStart = frameProfiler ? performance.now() : 0;
  let renderMs = 0, scriptsMs = 0, submitted = false;
  const stages = frameProfiler ? {} : null;
  try {
    if (active) {
      const bounds = canvas.getBoundingClientRect();
      if (bounds.width < 1 || bounds.height < 1) { requestAnimationFrame(tick); return; }
      const [outputWidth, outputHeight] = display_viewport_size(bounds.width, bounds.height, window.devicePixelRatio || 1, active.max_texture_dimension());
      const output = {width:outputWidth, height:outputHeight};
      const viewportChanged = output.width !== displayViewport.width || output.height !== displayViewport.height;
      if (viewportChanged) displayViewport = output;
      active.ui_set_width(bounds.width);
      active.resize(output.width,output.height);
      if (mode === 'menu' && menuLua && (active !== menuViewportOwner ||
          bounds.width !== menuViewportWidth || bounds.height !== menuViewportHeight)) {
        menuLua.dispatch('menu.viewport', {width:bounds.width, height:bounds.height});
        menuViewportOwner = active; menuViewportWidth = bounds.width; menuViewportHeight = bounds.height;
      }
      if (mode === 'menu' && menuLua && viewportChanged) menuLua.dispatch('menu.settings', menuSettings());
      // Render resolution, presentation and the viewport resources scene scripts read are applied in Rust.
      if (mode === 'gameplay' || mode === 'menu') {
        active.apply_display(displayPanel, bounds.width, bounds.height, output.width, output.height, mode === 'gameplay');
      }
      let earlyInputMs = 0;
      if (mode === 'gameplay' && gameplayLua) {
        const inputStart = frameProfiler ? performance.now() : 0;
        // Input updates camera and control state before this frame is drawn.
        // The remaining channels are still visited once, in their original order.
        gameplayLua.dispatchPending(gameplayInputs);
        inputLatency?.markDispatched();
        if (frameProfiler) earlyInputMs = performance.now() - inputStart;
      }
      const timedRenderer = stackFrame && stackCapture.detailed ? active : null;
      timedRenderer?.arm_frame_timing();
      const renderStart = frameProfiler ? performance.now() : 0;
      submitted = active.frame(timestamp);
      if (submitted && mode === 'gameplay') {
        // Capture before presented() advances a queued muzzle-flash frame.
        if (gameplayLua) publishFirstPersonPresentation(gameplayLua);
        reportWeaponFrame(firstPersonFrames?.presented());
        if (pendingLightPresentation !== null && gameplayLua) {
          // Acknowledge only a submitted frame; Lua owns the pulse and its fade.
          const id = pendingLightPresentation; pendingLightPresentation = null;
          gameplayLua.dispatch('lighting.flash_presented', {id});
          metrics.dataset.lightFlash = JSON.stringify({id, presented:true});
        }
      }
      if (submitted && frameProfiler && mode === 'gameplay') {
        voxelGame?.voxel_mark_frame(); inputLatency.markSubmitted();
      }
      if (frameProfiler) renderMs = performance.now() - renderStart;
      const rustTiming = timedRenderer ? JSON.parse(timedRenderer.take_frame_timing()) : null;
      if (active.ui_take_layout_changed()) saveLayout(active, layoutScene());
      if (mode === 'loading' && active.take_cancel_loading()) {
        cancelLoading();
      } else if (mode === 'selector') {
        const id = active.take_launch();
        const open = active.take_open_scene();
        if (id) void launchLevel(id);
        else if (open) { const request = JSON.parse(open); selectedSceneId = request.scene; void launchLevel(request.level, request.scene); }
      } else if (mode === 'gameplay' || mode === 'menu') {
        const scriptStart = frameProfiler ? performance.now() : 0;
        const runtime = mode === 'menu' ? menuLua : gameplayLua;
        const dt = lastScriptTime === null ? 0 : Math.max(0, Math.min((timestamp - lastScriptTime) / 1000, 0.25));
        lastScriptTime = timestamp;
        if (mode === 'gameplay') {
          const dispatchStart = frameProfiler ? performance.now() : 0;
          dispatchGameplayEvents(runtime);
          processSceneRequests(runtime);
          if (frameProfiler) stages.event_dispatch_ms = earlyInputMs + performance.now() - dispatchStart;
          if (mode === 'gameplay' && active) {
            const ticks = active.take_script_ticks();
            for (let i = 0; i < ticks; i++) {
              const luaStart = frameProfiler ? performance.now() : 0;
              runtime.run(active.script_tick_seconds());
              if (frameProfiler) stages.lua_systems_ms = (stages.lua_systems_ms || 0) + performance.now() - luaStart;
              const tickEventsStart = frameProfiler ? performance.now() : 0;
              // Deliver shots between fixed ticks so a burst never waits for the next frame.
              runtime.dispatchPending(tickEventChannels);
              if (frameProfiler) stages.tick_events_ms = (stages.tick_events_ms || 0) + performance.now() - tickEventsStart;
            }
            const outputStart = frameProfiler ? performance.now() : 0;
            runtime.consumePending(outputChannels, consumeGameplayOutput);
            firstPersonFrames?.update(timestamp);
            runtime.consumePending(trailingOutputChannels, consumeTrailingOutput);
            if (frameProfiler) stages.output_ms = performance.now() - outputStart;
            const voxelStart = frameProfiler ? performance.now() : 0;
            voxelGame?.voxel_tick(timestamp);
            processSceneRequests(runtime);
            if (frameProfiler) stages.voxel_ms = performance.now() - voxelStart;
          }
        } else {
          runtime.run(dt);
          runtime.dispatchPending(scriptEventChannels);
          processMenuSettings(runtime);
          processSceneRequests(runtime);
        }
        if (frameProfiler) scriptsMs = earlyInputMs + performance.now() - scriptStart;
      }
      if (timestamp - lastStatus >= 500) {
        if (mode === 'gameplay' && active) {
          const state = JSON.parse(active.status());
          if (frameProfiler) profileState = state;
          status.textContent = state.resident_splats ? 'Live WebGPU canvas' : 'Streaming scan';
          const timing = state.ui_frame_fps == null ? 'measuring frames' : `${state.ui_frame_fps.toFixed(1)} submitted fps · ${state.ui_frame_ms.toFixed(1)} ms/frame`;
          const sky = state.sky_loaded ? (state.sky_enabled ? 'HDR sky on' : 'HDR sky off') : (skyLoadState === 'error' ? 'HDR sky unavailable' : 'loading HDR sky');
          const lod = state.level_lod == null ? 'LOD auto' : `LOD ${state.level_lod}`;
          const finest = Array.isArray(state.leaves_per_lod) ? (state.leaves_per_lod[0] || 0) : 0;
          // Structured, read-only diagnostics help verify the streaming state during development.
          metrics.dataset.streaming = JSON.stringify({lods:state.leaves_per_lod, pending:state.lod_pending,
            wanted:state.lod_wanted_splats, activeSlots:state.lod_active_slots, free:state.lod_free_slots,
            largestFree:state.lod_largest_free, pendingDetails:state.lod_pending_details, loaded:state.lod_chunks_loaded,
            inFlight:state.lod_chunks_in_flight, fading:state.lod_fading, switches:state.lod_switches,
            fadeCuts:state.fade_cuts, hardSwaps:state.hard_swaps, octreeNodes:state.octree_nodes,
            decoders:state.decoders});
          let posture = null;
          const voxelPlayer = voxelGame?.voxel_player();
          if (voxelPlayer && gameplayScene.component_types?.PlayerPosture) {
            try { posture = JSON.parse(active.ecs_operation('gameplay', JSON.stringify({op:'get', id:voxelPlayer, name:'PlayerPosture'}))).value; }
            catch (error) { console.debug('Player posture diagnostics:', error); }
          }
          metrics.dataset.audio = JSON.stringify(sceneAudio.diagnostics());
          metrics.dataset.player = JSON.stringify({camera:state.player_camera, physics:state.player_physics, rig:state.player_rig, posture});
          metrics.dataset.display = JSON.stringify({...state.display, scaling:JSON.parse(displayPanel.preferences()).scaling});
          metrics.dataset.lens = JSON.stringify(state.camera_lens ?? null);
          // Recent ECS events make projectile timing and collision observable in development.
          const combatValue = name => JSON.parse(active.ecs_operation('gameplay', JSON.stringify({op:'get_resource', name}))).value ?? null;
          metrics.dataset.gameMode = JSON.stringify(combatValue('game.mode_state'));
          metrics.dataset.sprint = JSON.stringify({blur:combatValue('camera.peripheral_blur') ?? 0, combat_blur:combatValue('camera.combat_vision') ?? 0});
          metrics.dataset.weaponBlur = JSON.stringify(combatValue('render.weapon_motion_blur') ?? {x:0,y:0});
          metrics.dataset.combat = JSON.stringify({shot:combatValue('combat.last_shot'), impact:combatValue('combat.last_impact'),
            expiration:combatValue('combat.last_expiration')});
          const mouse = document.pointerLockElement === canvas ? 'Mouse locked · ~ releases' :
            mouseCapture.failed ? 'Drag to look' : 'Click scene to lock mouse';
          metrics.textContent = `${currentLevel.name} · ${state.resident_splats.toLocaleString()} splats · ${lod} · ${finest} LOD 0 regions · ${timing} · ${sky} · ${mouse}`;
        } else if (mode === 'menu') {
          status.textContent = 'Paused';
          metrics.textContent = `${currentLevel.name} · Paused`;
        } else if (mode === 'loading') {
          metrics.textContent = 'Preloading level assets';
        } else if (mode === 'selector') {
          metrics.textContent = `${catalog.levels.length} local levels · Choose a level and launch`;
        }
        if (timestamp >= skyErrorUntil) message.hidden = true;
        lastStatus = timestamp;
      }
      if (frameProfiler && mode === 'gameplay') {
        const callbackMs = performance.now() - profileStart;
        if (stackFrame) stackCapture.endFrame(submitted, renderMs, scriptsMs, stages, gameplayLua, rustTiming, timestamp);
        frameProfiler.record(timestamp, submitted, renderMs, scriptsMs, callbackMs, stages);
        if (timestamp - lastProfileReport >= 500) {
          const report = frameProfiler.snapshot(profileState, gameplayLua?.profileReport());
          report.voxel_worker = voxelGame?.voxel_latency_report() ?? null;
          report.mouse_input = inputLatency.snapshot();
          metrics.dataset.profile = JSON.stringify(report);
          lastProfileReport = timestamp;
        }
      }
    }
    requestAnimationFrame(tick);
  } catch (error) { showError(error); }
}
async function start() {
  if (!navigator.gpu) throw new Error('This browser does not expose WebGPU. Open the preview in a WebGPU-capable browser.');
  status.textContent = 'Loading level selector';
  // Host logic is Rust: initialise the module before anything calls into it.
  await init();
  sceneAudio = new SceneAudio();
  // Every third captured frame is detailed (and GPU-timed), so a capture reports up to 60 GPU samples.
  if (stackRequested) stackCapture = new FrameStackCapture(publishStackProfile,
    () => active && mode === 'gameplay' ? active.gpu_frame_timing() : null, 3);
  selectorUrl = new URL('./assets/scenes/level-selector.json', import.meta.url);
  selectorBytes = await getBytes(selectorUrl);
  const selectorScene = parseJson(selectorBytes);
  const resources = selectorScene.resources?.assets;
  if (!resources?.catalog?.uri || !resources?.gameplay_scene?.uri || !resources?.scene_catalog?.uri)
    throw new Error('Selector scene resources are missing.');
  defaultGameplayUrl = projectResourceUrl(resources.gameplay_scene.uri, new URL('.', import.meta.url).href, selectorUrl.href);
  sceneCatalogUrl = projectResourceUrl(resources.scene_catalog.uri, new URL('.', import.meta.url).href, selectorUrl.href);
  [catalogBytes, defaultGameplayBytes, sceneCatalogBytes] = await Promise.all([
    getBytes(projectResourceUrl(resources.catalog.uri, new URL('.', import.meta.url).href, selectorUrl.href)), getBytes(defaultGameplayUrl), getBytes(sceneCatalogUrl)]);
  catalog = parseJson(catalogBytes);
  if (loadPolicy(resources.catalog.load) !== 'preload' || loadPolicy(resources.gameplay_scene.load) !== 'preload' ||
      loadPolicy(resources.scene_catalog.load) !== 'preload')
    throw new Error('The selector catalogs and gameplay scene must be preloaded.');
  if (!globalThis.wasmoon?.LuaFactory) throw new Error('Lua 5.4 runtime did not load.');
  luaFactory = new globalThis.wasmoon.LuaFactory(new URL('./pkg/wasmoon/glue.wasm', import.meta.url).href);
  if (profileOptions.has('profile') || stackRequested) { frameProfiler = new FrameProfiler(); inputLatency = new InputFrameLatency(); }
  mouseCapture = new MouseCapture(canvas, document);
  gameplayMouse = new GameplayMouseButtons(pressed => gameplayLua?.emit('input.mouse_button', {button:'right', pressed, gameplay:true}));
  gameplayFire = new GameplayFireButton(pressed => gameplayLua?.emit('input.mouse_button', {button:'left', pressed, gameplay:true}));
  gameplayInputs = new EventChannels(gameplayInputChannels());
  gameplayEvents = new EventChannels(gameplayEventChannels());
  // Splat draws use GPU-written indirect arguments; `?indirect_draw=0` restores the older full-prefix direct draw.
  if (profileOptions.get('indirect_draw') === '0') set_indirect_draw(false);
  // `?sort_compaction=0` sorts every slot instead of only the splats in view (same picture; for A/B timing).
  if (profileOptions.get('sort_compaction') === '0') set_sort_compaction(false);
  // `?footprint_cull=0` keeps splats the vertex stage discards anyway in the sort and draw (same picture; A/B timing).
  if (profileOptions.get('footprint_cull') === '0') set_footprint_cull(false);
  active = await Selector.create(canvas, selectorBytes, catalogBytes, sceneCatalogBytes, '', '');
  displayPanel = new DisplaySettings();
  restoreLayout(active, 'selector');
  mode = 'selector';
  void populateSelectorPreviews(active);
  attachInput();
  document.addEventListener('visibilitychange', () => { if (document.hidden) resetPlayerInput(); active?.set_paused(document.hidden || mode === 'menu'); });
  message.hidden = true;
  status.textContent = 'Choose a level';
  requestAnimationFrame(tick);
}
start().catch(showError);
