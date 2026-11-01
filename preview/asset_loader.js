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
export function planScripts(scene, base, origin) {
  const scripts = scene.resources?.scripts || {};
  const explicit = scene.resources?.preview?.browser_scripts || [];
  if (!Array.isArray(explicit) || explicit.some(id => !Object.hasOwn(scripts, id))) throw new Error('Invalid browser script references.');
  return Object.entries(scripts).flatMap(([id, spec]) => {
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
