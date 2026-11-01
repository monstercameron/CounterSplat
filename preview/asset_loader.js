// Asset-loading policy and progress logic, independent of the browser renderer.
export function loadPolicy(value) {
  const policy = value ?? 'preload';
  if (policy !== 'preload' && policy !== 'streamable') throw new Error(`Unknown asset load policy: ${policy}`);
  return policy;
}
export function levelPolicies(value = {}) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid level asset policies.');
  const policies = {
    manifest: loadPolicy(value.manifest),
    viewer_settings: loadPolicy(value.viewer_settings),
    chunk_metadata: loadPolicy(value.chunk_metadata),
    splat_chunks: loadPolicy(value.splat_chunks ?? 'streamable'),
  };
  if (policies.manifest !== 'preload' || policies.viewer_settings !== 'preload' || policies.chunk_metadata !== 'preload')
    throw new Error('The level manifest, viewer settings, and chunk metadata must be preloaded.');
  return policies;
}
// Scene choices are filenames in the local project scene directory, never arbitrary URLs.
export function localSceneUrl(filename, origin) {
  if (typeof filename !== 'string' || !/^[A-Za-z0-9_-]{1,80}\.json$/.test(filename))
    throw new Error('Invalid scene filename.');
  return new URL(`/assets/scenes/${filename}`, origin);
}
export function planScripts(scene, base, origin) {
  const scripts = scene.resources?.scripts || {};
  const explicit = scene.resources?.preview?.browser_scripts || [];
  if (!Array.isArray(explicit) || explicit.some(id => typeof id !== 'string' || !Object.hasOwn(scripts, id)))
    throw new Error('Invalid browser script references.');
  // Execute explicitly ordered scene scripts in that order. Autoload and other
  // preloaded scripts not listed here follow in catalog order; no script runs twice.
  const ordered = [...new Set([...explicit, ...Object.keys(scripts)])];
  return ordered.flatMap(id => {
    const spec = scripts[id];
    const execute = Boolean(spec.autoload) || explicit.includes(id);
    const policy = loadPolicy(spec.load);
    if (execute && policy !== 'preload') throw new Error(`Required script ${id} must be preloaded.`);
    if (policy === 'streamable') return [];
    if (spec.language !== 'lua') throw new Error(`Script ${id} is not Lua.`);
    const url = new URL(spec.uri, base);
    if (url.origin !== origin) throw new Error(`Script ${id} must be local to this preview.`);
    return [{id, url, execute}];
  });
}

/// Report bounded, monotonic overall progress while retaining real per-phase counts.
export class LoadingProgress {
  constructor(report) { this.report = report; this.progress = 0; }
  update(phase, start, end, completed, total) {
    if (typeof phase !== 'string' || phase.length > 256 || !Number.isFinite(start) || !Number.isFinite(end) ||
        start < 0 || end > 1 || end < start || !Number.isInteger(completed) || !Number.isInteger(total) ||
        completed < 0 || total < 1 || completed > total) throw new Error('Invalid asset-loading progress.');
    this.progress = Math.max(this.progress, start + (end - start) * completed / total);
    this.report(phase, this.progress, completed, total);
  }
}

// Build a portable inventory from the actual level metadata and, when available, its download inventory.
// These are references only; the ECS does not own file bytes or GPU resources.
export function describeLevelAssets(level, lodCount, chunkMetadata, filenames, optional = {}, source = 'level') {
  if (!Number.isInteger(lodCount) || lodCount < 1 || lodCount > 64 || !Array.isArray(chunkMetadata) ||
      !Array.isArray(filenames) || filenames.length !== chunkMetadata.length) throw new Error('Invalid level asset metadata.');
  const policies = levelPolicies(level.assets);
  const files = new Map();
  const safe = path => typeof path === 'string' && path.length > 0 && path.length <= 1024 &&
    !path.startsWith('/') && !path.includes('\\') && !path.includes(':') &&
    !/[\x00-\x1f\x7f]/.test(path) && path.split('/').every(part => part && part !== '.' && part !== '..');
  const add = (path, kind, load) => {
    if (!safe(path)) throw new Error('A level asset has an invalid path.');
    if (!files.has(path)) files.set(path, {path, kind, load});
  };
  const single = level.format === 'single_sog';
  if (!single && level.format !== 'streamed_sog') throw new Error('Unsupported level format.');
  add(single ? 'meta.json' : 'lod-meta.json', 'manifest', policies.manifest);
  if (optional.viewerSettings) add('viewer-settings.json', 'viewer_settings', policies.viewer_settings);
  for (let index = 0; index < filenames.length; index++) {
    const metadataPath = single ? 'meta.json' : filenames[index];
    if (!single) add(metadataPath, 'chunk_metadata', policies.chunk_metadata);
    const folder = single ? '' : metadataPath.split('/').slice(0, -1).join('/') + '/';
    const meta = chunkMetadata[index];
    if (!meta || typeof meta !== 'object') throw new Error('Invalid chunk metadata.');
    for (const name of ['means', 'quats', 'scales', 'sh0', 'shN']) {
      const references = meta[name]?.files ?? [];
      if (!Array.isArray(references)) throw new Error('Invalid chunk asset list.');
      for (const path of references) {
        if (typeof path !== 'string') throw new Error('Invalid chunk asset path.');
        add(folder + path, 'splat_data', policies.splat_chunks);
      }
    }
  }
  if (optional.inventory) {
    if (optional.inventory.scene_id !== level.id || !Array.isArray(optional.inventory.files))
      throw new Error('Level download inventory does not match the selected level.');
    for (const item of optional.inventory.files) {
      if (!item || typeof item.path !== 'string') throw new Error('Invalid level inventory entry.');
      add(item.path, /(?:^|\/)scene\.voxel\./.test(item.path) ? 'collision' : 'other', 'streamable');
    }
    add('download-inventory.json', 'other', 'preload');
  }
  if (optional.source) add('source.json', 'other', 'preload');
  return {source, level_id:level.id, name:level.name, format:level.format, lod_count:lodCount, files:[...files.values()]};
}
