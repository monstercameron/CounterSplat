// The scene owns assets and Lua owns playback decisions. This browser host only
// decodes preloaded clips and mixes requested voices; it never fetches from events.
const finite = (n, lo, hi) => typeof n === 'number' && Number.isFinite(n) && n >= lo && n <= hi;
export class SceneAudio {
  constructor(createContext = () => new AudioContext()) {
    this.createContext = createContext; this.context = null; this.master = null;
    this.encoded = new Map(); this.clips = new Map(); this.layers = new Map();
    this.voices = new Set(); this.generation = 0; this.played = 0; this.decoding = new Map();
  }
  // Call synchronously from a real pointer/key gesture to satisfy autoplay policy.
  unlock() {
    try {
      if (!this.context) {
        this.context = this.createContext(); this.master = this.context.createGain();
        this.master.gain.value = 0.8; this.master.connect(this.context.destination);
        this.applyLayers();
      }
      const resumed = this.context.state === 'suspended' ? this.context.resume() : Promise.resolve();
      Promise.resolve(resumed).then(() => this.decode()).catch(error => console.warn('Audio unlock failed:', error));
    } catch (error) { console.warn('Audio is unavailable:', error); }
  }
  async load(scene, base, cache) {
    this.clear();
    for (const [id, asset] of Object.entries(scene.resources?.assets || {})) {
      if (asset.kind !== 'sound' || (asset.load ?? 'preload') !== 'preload') continue;
      const bytes = cache.get(new URL(asset.uri, base).href);
      if (bytes) this.encoded.set(id, bytes);
    }
    await this.decode();
  }
  async decode() {
    if (!this.context) return;
    const generation = this.generation;
    await Promise.all([...this.encoded].map(([id, bytes]) => {
      if (this.clips.has(id)) return Promise.resolve();
      if (this.decoding.has(id)) return this.decoding.get(id);
      const task = (async () => {
        try {
          const data = bytes instanceof ArrayBuffer ? bytes.slice(0) : bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
          const clip = await this.context.decodeAudioData(data);
          if (generation === this.generation && (clip.numberOfChannels === 1 || clip.numberOfChannels === 2)) this.clips.set(id, clip);
        } catch (error) { if (generation === this.generation) console.warn(`Could not decode sound ${id}:`, error); }
        finally { if (generation === this.generation) this.decoding.delete(id); }
      })();
      this.decoding.set(id, task); return task;
    }));
  }
  setLayer(event) {
    if (typeof event?.name !== 'string' || !event.name || event.name.length > 64) return;
    const mix = event.mix;
    if (!mix || !finite(mix.gain, 0, 2) || typeof mix.muted !== 'boolean' || typeof mix.solo !== 'boolean' || typeof event.enabled !== 'boolean') return;
    let layer = this.layers.get(event.name);
    if (!layer) { layer = {node:null}; this.layers.set(event.name, layer); }
    Object.assign(layer, {gain:mix.gain, muted:mix.muted, solo:mix.solo, enabled:event.enabled});
    this.applyLayers();
  }
  applyLayers() {
    if (!this.context) return;
    const solo = [...this.layers.values()].some(item => item.enabled && item.solo);
    for (const item of this.layers.values()) {
      if (!item.node) { item.node = this.context.createGain(); item.node.connect(this.master); }
      item.node.gain.value = item.enabled && !item.muted && (!solo || item.solo) ? item.gain : 0;
    }
  }
  play(event) {
    const clip = this.clips.get(event?.asset), layer = this.layers.get(event?.layer), settings = event?.settings;
    if (!this.context || this.context.state !== 'running' || !clip || !layer || !settings ||
        !finite(settings.gain, 0, 2) || !finite(settings.balance, -1, 1) ||
        !finite(settings.rate, .25, 4) || typeof settings.looped !== 'boolean') return false;
    if (this.voices.size >= 32) this.stopVoice(this.voices.values().next().value);
    const source = this.context.createBufferSource(), split = this.context.createChannelSplitter(2);
    const left = this.context.createGain(), right = this.context.createGain(), merge = this.context.createChannelMerger(2);
    const angle = (settings.balance + 1) * Math.PI / 4;
    left.gain.value = settings.gain * Math.SQRT2 * Math.cos(angle);
    right.gain.value = settings.gain * Math.SQRT2 * Math.sin(angle);
    source.buffer = clip; source.loop = settings.looped; source.playbackRate.value = settings.rate;
    source.connect(split); split.connect(left, 0); split.connect(right, clip.numberOfChannels === 1 ? 0 : 1);
    left.connect(merge, 0, 0); right.connect(merge, 0, 1); merge.connect(layer.node);
    const voice = {source, nodes:[source, split, left, right, merge]};
    source.onended = () => this.dispose(voice); this.voices.add(voice); source.start(); this.played++;
    return true;
  }
  dispose(voice) {
    if (!this.voices.delete(voice)) return;
    voice.source.onended = null;
    for (const node of voice.nodes) node.disconnect();
  }
  stopVoice(voice) { if (!voice) return; try { voice.source.stop(); } catch {} this.dispose(voice); }
  stopVoices() { for (const voice of [...this.voices]) this.stopVoice(voice); }
  clear() {
    this.generation++; this.stopVoices(); this.encoded.clear(); this.clips.clear(); this.decoding.clear();
    for (const layer of this.layers.values()) layer.node?.disconnect(); this.layers.clear();
  }
  diagnostics() { return {state:this.context?.state ?? 'locked', clips:this.clips.size, voices:this.voices.size, played:this.played}; }
}
