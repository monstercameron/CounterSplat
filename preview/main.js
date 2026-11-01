// Browser host: both the selector and gameplay render directly to the WebGPU canvas.
import init, { Selector, Viewer } from '/pkg/countersplat_web.js';
import { LuaScene } from '/lua_runtime.js';
import { LoadingProgress, levelPolicies, loadPolicy, planScripts } from '/asset_loader.js';

const canvas = document.getElementById('canvas');
const status = document.getElementById('status');
const message = document.getElementById('message');
const metrics = document.getElementById('metrics');
const arrangeButton = document.getElementById('arrange');
const resetLayoutButton = document.getElementById('reset-layout');
const inFlight = new Set();
let active = null;
let arrangeMode = false;
let mode = 'boot';
let generation = 0;
let lastStatus = 0;
let stopped = false;
let selectorBytes, catalogBytes, gameplayBytes, menuBytes, catalog, gameplayScene, menuScene;
let selectorUrl, gameplayUrl, menuUrl, engineApi, gameplayScripts, menuScripts, luaFactory;
let gameplayLua = null, menuLua = null, lastScriptTime = null;
let dragLook = false, hadPointerLock = false, expectedUnlock = false, lastEscapeTime = -Infinity;
let pointerLockAvailable = true;
const playerControlsKey = 'countersplat.player-controls.v1';
let loadingAbort = null, preloadedSceneAssets = new Map();
const MAX_PRELOAD_BYTES = 512 * 1024 * 1024;
let selectedId = '';
let currentLevel = null;
let levelBase = null;
let chunkMetas = [];
let decoder = null;
let skyDecoder = null;
let skyRequest = 0;
let skyLoadState = 'loading';
let skyErrorUntil = 0;
let localSkyId = 'preview-local-0';

// Scene definitions remain JSON; browser-local offsets are optional user preferences.
function layoutKey(scene) { return `countersplat.ui-layout.${scene}.v1`; }
function restoreLayout(target, scene) {
  try {
    const saved = localStorage.getItem(layoutKey(scene));
    if (saved !== null) target.ui_restore_layout(saved);
  } catch (error) { console.warn('Could not restore UI layout:', error); }
}
function saveLayout(target, scene) {
  try { localStorage.setItem(layoutKey(scene), target.ui_layout()); }
  catch (error) { console.warn('Could not save UI layout:', error); }
}
function layoutScene() { return mode === 'menu' ? 'menu' : mode === 'gameplay' ? 'gameplay' : 'selector'; }
function resetPlayerInput() { if (gameplayLua) gameplayLua.emit('input.reset', {}); }
function releaseMouseLook() {
  dragLook = false;
  if (document.pointerLockElement === canvas) { expectedUnlock = true; document.exitPointerLock(); }
}
function restorePlayerControls(game) {
  try {
    const raw = localStorage.getItem(playerControlsKey);
    if (!raw) return;
    if (raw.length > 16384) throw new Error('Saved controls are too large.');
    const ids = JSON.parse(game.ecs_operation('gameplay', JSON.stringify({op:'get_resource',name:'scene.entities'}))).value;
    game.ecs_operation('gameplay', JSON.stringify({op:'set',id:ids.player,name:'PlayerControls',value:JSON.parse(raw)}));
  } catch (error) { console.warn('Could not restore player controls:', error); }
}
function savePlayerControls(settings) {
  try { localStorage.setItem(playerControlsKey, JSON.stringify(settings)); }
  catch (error) { console.warn('Could not save player controls:', error); }
}
function dispatchGameplayEvents(runtime) {
  for (const name of ['input.key', 'input.mouse_delta', 'input.reset', 'ui.action']) {
    for (const event of runtime.drain(name)) runtime.dispatch(name, event);
  }
}
function closeScripts() {
  gameplayLua?.close(); menuLua?.close();
  gameplayLua = null; menuLua = null; lastScriptTime = null;
}
function sceneAsset(scene, base, id) {
  const asset = scene.resources?.assets?.[id];
  if (!asset || asset.kind !== 'scene') throw new Error(`Unknown scene asset ${id}.`);
  const url = new URL(asset.uri, base);
  if (url.origin !== location.origin) throw new Error('Scene asset must be local to this preview.');
  return url.href;
}
function processSceneRequests(runtime) {
  for (const request of runtime.drain('engine.scene')) {
    if (mode === 'gameplay' && request.action === 'push' && sceneAsset(gameplayScene, gameplayUrl, request.scene) === menuUrl.href) {
      runtime.dispatch('input.reset', {});
      releaseMouseLook();
      active.ui_focus(false); active.ui_pointer_gone();
      active.set_menu_visible(true); active.set_paused(true); mode = 'menu';
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
async function preloadScripts(gameScene, menu, signal, tracker, cache) {
  const gamePlan = planScripts(gameScene, gameplayUrl, location.origin);
  const menuPlan = planScripts(menu, menuUrl, location.origin);
  const tasks = [...gamePlan.map(item => ({...item, scope:'game'})), ...menuPlan.map(item => ({...item, scope:'menu'})),
    {id:'engine-api', url:new URL('/pkg/engine_api.lua', location.href), scope:'engine', execute:true}];
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
async function preloadSkies(scene, cache, signal, tracker) {
  const skies = Object.entries(scene.resources?.assets || {}).filter(([, asset]) => asset.kind === 'radiance_hdr' && loadPolicy(asset.load) === 'preload');
  if (skies.length > 8) throw new Error('The preview supports at most eight skybox assets.');
  tracker.update('Decoding preloaded skies', 0.75, 0.85, 0, Math.max(skies.length, 1));
  if (!skies.length) { tracker.update('Decoding preloaded skies', 0.75, 0.85, 1, 1); return []; }
  const worker = new Worker('/pkg/skybox-worker.js', {type:'module'});
  try {
    const prepared = [];
    for (const [id, asset] of skies) {
      const url = new URL(asset.uri, gameplayUrl);
      const bytes = cache.get(url.href);
      if (!bytes) throw new Error(`Preloaded sky ${id} is missing.`);
      const data = await requestWorker(worker, {request:0, asset:id, select:false, bytes:bytes.slice()}, signal);
      prepared.push({id, size:data.size, pixels:data.pixels});
      tracker.update('Decoding preloaded skies', 0.75, 0.85, prepared.length, skies.length);
    }
    return prepared;
  } finally { worker.terminate(); }
}
async function preloadChunks(metas, filenames, single, base, signal, tracker) {
  const worker = new Worker('/decoder.js', {type:'module'});
  const chunks = []; let bytes = 0;
  tracker.update('Decoding splat chunks', 0.85, 0.96, 0, Math.max(metas.length, 1));
  try {
    for (let index = 0; index < metas.length; index++) {
      const folder = single ? 'single' : filenames[index].split('/')[0];
      const data = await requestWorker(worker, {index, folder, metadata:metas[index], base:base.href, single}, signal);
      bytes += data.metadata.byteLength + data.dimensions.byteLength + data.pixels.reduce((sum, pixels) => sum + pixels.byteLength, 0);
      if (bytes > MAX_PRELOAD_BYTES) throw new Error('Decoded splat preload exceeds 512 MiB. Mark splat chunks streamable.');
      chunks.push(data);
      tracker.update('Decoding splat chunks', 0.85, 0.96, chunks.length, metas.length);
    }
    return chunks;
  } finally { worker.terminate(); }
}
function parseJson(bytes) { return JSON.parse(new TextDecoder().decode(bytes)); }
function localLevelPath(relative) {
  if (typeof relative !== 'string' || !relative || relative.startsWith('/') || relative.includes('\\') ||
      relative.split('/').some(part => !part || part === '.' || part === '..')) {
    throw new Error('The level contains an invalid asset path.');
  }
  return new URL(relative.split('/').map(encodeURIComponent).join('/'), levelBase).href;
}
function retryChunk(index, error, game, token) {
  console.warn(`Chunk ${index} failed:`, error);
  setTimeout(() => {
    if (generation === token && active === game && (mode === 'gameplay' || mode === 'menu')) {
      game.chunk_failed(index);
      inFlight.delete(index);
    }
  }, 3000);
}
function requestChunk(index, folder, metadata, game, token) {
  inFlight.add(index);
  if (!metadata) { retryChunk(index, 'metadata is unavailable', game, token); return; }
  try { decoder.postMessage({index, folder, metadata, base: levelBase.href, single: currentLevel.format === 'single_sog'}); }
  catch (error) { retryChunk(index, error, game, token); }
}
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
  skyDecoder = new Worker('/pkg/skybox-worker.js', {type: 'module'});
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
  if (decoder) decoder.terminate();
  if (skyDecoder) skyDecoder.terminate();
  decoder = null;
  skyDecoder = null;
  inFlight.clear();
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
async function launchLevel(id) {
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
    const policies = levelPolicies(selected.assets);
    if (loadPolicy(gameplayScene.resources?.assets?.pause_menu?.load) !== 'preload')
      throw new Error('The gameplay pause menu must be preloaded by this host.');
    levelBase = new URL(`/assets/levels/${encodeURIComponent(id)}/`, location.href);
    tracker.update('Reading level manifest', 0, 0.1, 0, 2);
    let manifestTasks = 0;
    const fetchManifest = async (path, optional = false) => {
      const bytes = await getBytes(path, signal, optional);
      tracker.update('Reading level manifest', 0, 0.1, ++manifestTasks, 2);
      return bytes;
    };
    const [primary, settings] = await Promise.all([
      fetchManifest(new URL(selected.format === 'single_sog' ? 'meta.json' : 'lod-meta.json', levelBase)),
      fetchManifest(new URL('viewer-settings.json', levelBase), true),
    ]);
    let lodBytes = primary, metas, filenames;
    let capacity = gameplayScene.resources?.preview?.capacity;
    if (!Number.isInteger(capacity) || capacity <= 0) throw new Error('Gameplay scene capacity is invalid.');
    if (selected.format === 'single_sog') {
      const synthetic = singleSogManifest(parseJson(primary));
      lodBytes = new TextEncoder().encode(JSON.stringify(synthetic));
      metas = [primary]; filenames = synthetic.filenames;
      capacity = Math.max(capacity, Math.ceil(synthetic.counts[0] * 100 / 85) + 1);
      tracker.update('Reading chunk metadata', 0.1, 0.5, 1, 1);
    } else if (selected.format === 'streamed_sog') {
      const lod = parseJson(primary);
      if (!Array.isArray(lod.filenames) || !lod.filenames.length || lod.filenames.length > 10_000)
        throw new Error('The level does not contain a valid chunk list.');
      filenames = lod.filenames;
      let done = 0;
      tracker.update('Reading chunk metadata', 0.1, 0.5, 0, filenames.length);
      metas = await mapLimit(filenames, 12, async path => {
        const bytes = await getBytes(localLevelPath(path), signal);
        tracker.update('Reading chunk metadata', 0.1, 0.5, ++done, filenames.length);
        return bytes;
      });
    } else throw new Error('This level format is not supported.');
    if (generation !== token) return;

    const sceneAssets = new Map([[selectorUrl.href, selectorBytes], [gameplayUrl.href, gameplayBytes]]);
    await preloadSceneAssets(gameplayScene, gameplayUrl, sceneAssets, signal, tracker, 0.5, 0.6);
    menuBytes = sceneAssets.get(menuUrl.href);
    if (!menuBytes) throw new Error('The preloaded pause menu is missing.');
    menuScene = parseJson(menuBytes);
    await preloadSceneAssets(menuScene, menuUrl, sceneAssets, signal, tracker, 0.6, 0.65);
    const scripts = await preloadScripts(gameplayScene, menuScene, signal, tracker, sceneAssets);
    const skies = await preloadSkies(gameplayScene, sceneAssets, signal, tracker);
    const chunks = policies.splat_chunks === 'preload'
      ? await preloadChunks(metas, filenames, selected.format === 'single_sog', levelBase, signal, tracker)
      : (tracker.update('Splat chunks will stream in the scene', 0.85, 0.96, 1, 1), []);
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
    pendingGame = await Viewer.create(canvas, lodBytes, metas, settings, [gameplayBytes, menuBytes], capacity, selected.rotate_z_180 !== false, policies.splat_chunks === 'preload');
    if (generation !== token) { pendingGame.free(); pendingGame = null; return; }
    for (const sky of skies) pendingGame.install_skybox(sky.id, sky.size, sky.pixels);
    for (const chunk of chunks) pendingGame.insert_decoded_chunk(chunk.index, chunk.metadata, chunk.dimensions, chunk.pixels);
    restorePlayerControls(pendingGame);
    gameplayScripts = scripts.gameplay; menuScripts = scripts.menu; engineApi = scripts.engine;
    gameplayLua = await LuaScene.create(luaFactory, pendingGame, 'gameplay', engineApi, gameplayScripts);
    menuLua = await LuaScene.create(luaFactory, pendingGame, 'menu', engineApi, menuScripts);
    if (generation !== token) { closeScripts(); pendingGame.free(); pendingGame = null; return; }
    active = pendingGame;
    pendingGame = null;
    restoreLayout(active, 'gameplay');
    active.ui_set_arrange_mode(arrangeMode);
    currentLevel = selected;
    chunkMetas = metas;
    preloadedSceneAssets = sceneAssets;
    inFlight.clear();
    mode = 'gameplay';
    const game = active;
    decoder = new Worker('/decoder.js', {type: 'module'});
    decoder.onmessage = ({data}) => {
      if (generation !== token || active !== game || (mode !== 'gameplay' && mode !== 'menu')) return;
      if (!data.ok) { retryChunk(data.index, data.error, game, token); return; }
      try {
        game.insert_decoded_chunk(data.index, data.metadata, data.dimensions, data.pixels);
        inFlight.delete(data.index);
      } catch (error) { retryChunk(data.index, error, game, token); }
    };
    decoder.onerror = event => { if (generation === token) showError(new Error(`Asset worker failed: ${event.message}`)); };
    startSkyWorker(game, token, new Set(skies.map(sky => sky.id)));
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
      try { active = await Selector.create(canvas, selectorBytes, catalogBytes, selectedId); }
      catch (restoreError) { showError(restoreError); return; }
    } else {
      active.finish_loading('failed');
    }
    mode = 'selector';
    restoreLayout(active, 'selector');
    active.ui_set_arrange_mode(arrangeMode);
    active.set_busy(false);
    active.set_notice(`Could not load level: ${error instanceof Error ? error.message : String(error)}`);
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
    active = await Selector.create(canvas, selectorBytes, catalogBytes, selectedId);
    restoreLayout(active, 'selector');
    active.ui_set_arrange_mode(arrangeMode);
    mode = 'selector';
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
  canvas.addEventListener('focus', () => send(scene => scene.ui_focus(true)));
  canvas.addEventListener('blur', () => { resetPlayerInput(); send(scene => { scene.ui_focus(false); scene.ui_pointer_gone(); }); });
  canvas.addEventListener('pointermove', event => {
    if (mode === 'gameplay' && (document.pointerLockElement === canvas || dragLook)) {
      if (Number.isFinite(event.movementX) && Number.isFinite(event.movementY))
        gameplayLua?.emit('input.mouse_delta', {dx:event.movementX, dy:event.movementY});
      return;
    }
    send(scene => { const [x,y] = point(event); scene.ui_pointer_move(x,y); });
  });
  canvas.addEventListener('pointerdown', event => send(scene => {
    const which = button(event); if (which === undefined) return;
    event.preventDefault(); canvas.focus({preventScroll:true});
    const [x,y] = point(event);
    const world = mode === 'gameplay' && !arrangeMode && !scene.ui_hit_test(x,y) && scene.player_mouse_look_enabled();
    if (world && which === 0 && event.pointerType === 'mouse' && document.pointerLockElement !== canvas) {
      // Drag-to-look remains available when an embedded browser does not support pointer lock.
      dragLook = true; canvas.setPointerCapture(event.pointerId); scene.ui_pointer_gone();
      if (pointerLockAvailable && canvas.requestPointerLock) {
        try { Promise.resolve(canvas.requestPointerLock()).catch(() => { pointerLockAvailable = false; }); }
        catch (_) { pointerLockAvailable = false; }
      }
      return;
    }
    if (world && which === 1 && event.pointerType === 'mouse') { dragLook = true; canvas.setPointerCapture(event.pointerId); return; }
    if (document.pointerLockElement === canvas) return;
    canvas.setPointerCapture(event.pointerId);
    scene.ui_pointer_move(x,y); scene.ui_pointer_button(x,y,which,true,modifiers(event));
  }));
  canvas.addEventListener('pointerup', event => send(scene => {
    const which = button(event); if (which === undefined) return;
    if (dragLook) { dragLook = false; return; }
    if (document.pointerLockElement === canvas) return;
    const [x,y] = point(event); scene.ui_pointer_button(x,y,which,false,modifiers(event));
    if (event.pointerType !== 'mouse') scene.ui_pointer_gone();
  }));
  canvas.addEventListener('pointercancel', event => send(scene => {
    dragLook = false;
    const [x,y] = point(event);
    for (const which of [0,1,2]) scene.ui_pointer_button(x,y,which,false,modifiers(event));
    scene.ui_pointer_gone();
  }));
  canvas.addEventListener('pointerleave', event => { if (!event.buttons) send(scene => scene.ui_pointer_gone()); });
  canvas.addEventListener('wheel', event => {
    if (active?.ui_wants_pointer()) { event.preventDefault(); send(scene => scene.ui_scroll(-event.deltaX,-event.deltaY)); }
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
    const script = mode === 'gameplay' ? gameplayLua : mode === 'menu' ? menuLua : null;
    if (!script || event.isComposing || event.key.length > 128) return;
    const gameplay = mode === 'gameplay' && document.activeElement === canvas && !active.ui_wants_keyboard() &&
      !arrangeMode && !document.hidden && !event.ctrlKey && !event.metaKey && !event.altKey;
    script.emit('input.key', {key: event.key, code: event.code, pressed: event.type === 'keydown', is_repeat: event.repeat, gameplay});
    if (event.key === 'Escape') { if (event.type === 'keydown') lastEscapeTime = performance.now(); event.preventDefault(); }
    else if (gameplay) event.preventDefault();
  };
  window.addEventListener('keydown', gameKey);
  window.addEventListener('keyup', gameKey);
  window.addEventListener('blur', resetPlayerInput);
  document.addEventListener('pointerlockchange', () => {
    if (document.pointerLockElement === canvas) { hadPointerLock = true; return; }
    if (!hadPointerLock) return;
    hadPointerLock = false;
    resetPlayerInput();
    if (expectedUnlock) { expectedUnlock = false; return; }
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
  try {
    if (active) {
      const bounds = canvas.getBoundingClientRect();
      const scale = Math.min(window.devicePixelRatio || 1, 1.5);
      const width = Math.max(1, Math.min(1280, Math.round(bounds.width * scale)));
      const height = Math.max(1, Math.min(720, Math.round(width * 9 / 16)));
      active.ui_set_width(bounds.width);
      active.resize(width,height);
      active.frame(timestamp);
      if (active.ui_take_layout_changed()) saveLayout(active, layoutScene());
      if (mode === 'loading' && active.take_cancel_loading()) {
        cancelLoading();
      } else if (mode === 'selector') {
        const id = active.take_launch();
        if (id) void launchLevel(id);
      } else if (mode === 'gameplay' || mode === 'menu') {
        const runtime = mode === 'menu' ? menuLua : gameplayLua;
        const dt = lastScriptTime === null ? 0 : Math.max(0, Math.min((timestamp - lastScriptTime) / 1000, 0.25));
        lastScriptTime = timestamp;
        if (mode === 'gameplay') {
          dispatchGameplayEvents(runtime);
          processSceneRequests(runtime);
          if (mode === 'gameplay' && active) {
            const ticks = active.take_script_ticks();
            for (let i = 0; i < ticks; i++) runtime.run(active.script_tick_seconds());
            for (const settings of runtime.drain('player.controls_changed')) savePlayerControls(settings);
            processSceneRequests(runtime);
          }
        } else {
          runtime.run(dt);
          processSceneRequests(runtime);
        }
        if (mode === 'gameplay' && active) {
          const game = active, token = generation;
          for (const [index, folder] of JSON.parse(game.requests())) {
            if (!inFlight.has(index)) requestChunk(index, folder, chunkMetas[index], game, token);
          }
        }
      }
      if (timestamp - lastStatus >= 500) {
        if (mode === 'gameplay' && active) {
          const state = JSON.parse(active.status());
          status.textContent = state.resident_splats ? 'Live WebGPU canvas' : 'Streaming scan';
          const timing = state.ui_frame_fps == null ? 'measuring frames' : `${state.ui_frame_fps.toFixed(1)} submitted fps · ${state.ui_frame_ms.toFixed(1)} ms/frame`;
          const sky = state.sky_loaded ? (state.sky_enabled ? 'HDR sky on' : 'HDR sky off') : (skyLoadState === 'error' ? 'HDR sky unavailable' : 'loading HDR sky');
          metrics.textContent = `${currentLevel.name} · ${state.resident_splats.toLocaleString()} splats · ${timing} · ${sky}`;
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
    }
    requestAnimationFrame(tick);
  } catch (error) { showError(error); }
}
async function start() {
  if (!navigator.gpu) throw new Error('This browser does not expose WebGPU. Open the preview in a WebGPU-capable browser.');
  status.textContent = 'Loading level selector';
  selectorUrl = new URL('/assets/scenes/level-selector.json', location.href);
  selectorBytes = await getBytes(selectorUrl);
  const selectorScene = parseJson(selectorBytes);
  const resources = selectorScene.resources?.assets;
  if (!resources?.catalog?.uri || !resources?.gameplay_scene?.uri) throw new Error('Selector scene resources are missing.');
  gameplayUrl = new URL(resources.gameplay_scene.uri, selectorUrl);
  [catalogBytes, gameplayBytes] = await Promise.all([getBytes(new URL(resources.catalog.uri, selectorUrl)), getBytes(gameplayUrl)]);
  catalog = parseJson(catalogBytes);
  gameplayScene = parseJson(gameplayBytes);
  menuUrl = new URL(sceneAsset(gameplayScene, gameplayUrl, 'pause_menu'));
  if (loadPolicy(resources.catalog.load) !== 'preload' || loadPolicy(resources.gameplay_scene.load) !== 'preload')
    throw new Error('The selector catalog and gameplay scene must be preloaded.');
  if (!globalThis.wasmoon?.LuaFactory) throw new Error('Lua 5.4 runtime did not load.');
  luaFactory = new globalThis.wasmoon.LuaFactory('/pkg/wasmoon/glue.wasm');
  await init();
  active = await Selector.create(canvas, selectorBytes, catalogBytes, '');
  restoreLayout(active, 'selector');
  mode = 'selector';
  attachInput();
  document.addEventListener('visibilitychange', () => { if (document.hidden) resetPlayerInput(); active?.set_paused(document.hidden || mode === 'menu'); });
  message.hidden = true;
  status.textContent = 'Choose a level';
  requestAnimationFrame(tick);
}
start().catch(showError);
