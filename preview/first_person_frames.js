// Stream scene-declared image frames into the canvas renderer. Lua owns state,
// frame choice and playback timing; this class only resolves, decodes and caches.
export class FirstPersonFrames {
  constructor(scene, base, manifest, preloaded, viewer) {
    this.assets = scene.resources.assets; this.base = base; this.clips = manifest;
    this.preloaded = preloaded; this.viewer = viewer;
    this.cache = new Map(); this.pending = new Set(); this.queue = [];
    this.failed = new Set(); this.abort = new AbortController(); this.closed = false;
    this.active = 0; this.limit = 4; this.maxCache = 48; this.desired = null; this.previous = null;
    // Decode the very short muzzle flashes before the longer raise animation.
    // They remain resident so a shot can show its first visible frame immediately.
    this.pinned = new Set();
    for (const clip of [manifest.fireHip, manifest.fireADS])
      for (const id of (clip?.frames || []).slice(0, 2)) this.pinned.add(id);
    for (const id of (manifest.raising?.frames || [])) this.pinned.add(id);
    this.uploaded = null; this.uploadedClip = null; this.uploadedIndex = null;
    this.desiredClip = null; this.desiredIndex = null; this.presentationSettings = null;
    this.blendFrom = null; this.presentationBlend = 1;
    this.awaitingFlash = null; this.afterFlash = null;
    this.transitionSeconds = 0; this.blendElapsed = 0; this.blendDuration = 0;
    this.lastTime = null; this.wrapPending = false; this.motion = {x:0,y:0,roll:0};
    // Warm the reversible raise and brief firing takes before they are needed.
    // The remaining firing frames may be evicted to make room for longer clips.
    for (const id of [...this.pinned, ...(manifest.fireHip?.frames || []), ...(manifest.fireADS?.frames || [])])
      this.enqueue(id, false);
    this.pump();
  }
  enqueue(id, priority) {
    if (!id || this.cache.has(id) || this.pending.has(id) || this.failed.has(id)) return;
    const existing = this.queue.indexOf(id);
    if (existing >= 0) { if (!priority) return; this.queue.splice(existing, 1); }
    if (priority) this.queue.unshift(id); else this.queue.push(id);
    if (this.queue.length > 96) this.queue.length = 96;
  }
  pump() {
    while (!this.closed && this.active < this.limit && this.queue.length) {
      const id = this.queue.shift();
      if (this.cache.has(id) || this.pending.has(id) || this.failed.has(id)) continue;
      this.pending.add(id); this.active++;
      this.load(id).finally(() => { this.pending.delete(id); this.active--; this.pump(); });
    }
  }
  async load(id) {
    let bitmap;
    try {
      const asset = this.assets[id];
      if (!asset || asset.kind !== 'first_person_frame') throw new Error(`Undeclared first-person frame ${id}`);
      const url = new URL(asset.uri, this.base);
      if (url.origin !== location.origin) throw new Error('First-person frames must be local scene assets');
      let bytes = this.preloaded.get(url.href);
      if (!bytes) {
        const response = await fetch(url, {signal:this.abort.signal});
        if (!response.ok) throw new Error(`HTTP ${response.status} for ${url.pathname}`);
        const blob = await response.blob();
        bitmap = await createImageBitmap(blob, {premultiplyAlpha:'none'});
      } else bitmap = await createImageBitmap(new Blob([bytes], {type:'image/webp'}), {premultiplyAlpha:'none'});
      if (this.closed) { bitmap.close(); return; }
      this.cache.set(id, bitmap);
      this.evict();
      if (this.desired === id) this.display(id, bitmap);
    } catch (error) {
      if (!this.closed && error.name !== 'AbortError') { this.failed.add(id); console.warn('First-person frame:', error); }
      if (bitmap) { this.cache.delete(id); bitmap.close(); }
    }
  }
  evict() {
    while (this.cache.size > this.maxCache) {
      const id = [...this.cache.keys()].find(key => key !== this.desired && !this.pinned.has(key));
      if (!id) break;
      this.cache.get(id).close(); this.cache.delete(id);
    }
  }
  display(id, bitmap) {
    if (this.uploaded === id) { this.uploadedClip = this.desiredClip; this.uploadedIndex = this.desiredIndex; this.wrapPending = false; return; }
    if (this.uploaded && (this.uploadedClip !== this.desiredClip || this.wrapPending)) {
      this.blendDuration = this.transitionSeconds;
      this.blendElapsed = 0;
      const previous = {clip:this.uploadedClip,index:this.uploadedIndex};
      if (!this.blendDuration || !this.viewer.capture_first_person_frame()) this.blendDuration = 0;
      this.blendFrom = this.blendDuration ? previous : null;
    }
    this.viewer.install_first_person_frame(bitmap);
    this.uploaded = id; this.uploadedClip = this.desiredClip; this.uploadedIndex = this.desiredIndex; this.wrapPending = false;
    this.applyMotion();
  }
  applyMotion() {
    const progress = this.blendDuration > 0 ? Math.min(1, this.blendElapsed / this.blendDuration) : 1;
    const smooth = progress * progress * (3 - 2 * progress);
    this.presentationBlend = smooth;
    this.viewer.set_first_person_motion(this.motion.x, this.motion.y, this.motion.roll, smooth);
    if (progress >= 1) { this.blendDuration = 0; this.blendFrom = null; }
  }
  // Presentation metadata describes the texture actually installed, not a
  // newer request that may still be decoding. Lua owns the HUD's behavior.
  presentation() {
    if (!this.presentationSettings || !this.uploadedClip || !this.uploadedIndex) return {visible:false};
    return {...this.presentationSettings,clip:this.uploadedClip,index:this.uploadedIndex,
      motion_x:this.motion.x,motion_y:this.motion.y,motion_roll:this.motion.roll,
      blend:this.presentationBlend,previous:this.blendFrom};
  }
  // Only presentation interpolation lives here; Lua chooses clips and durations.
  update(now) {
    if (this.closed || !Number.isFinite(now)) return;
    const elapsed = this.lastTime === null || now < this.lastTime ? 0 : Math.min(50, now - this.lastTime);
    this.lastTime = now;
    if (this.blendDuration > 0) { this.blendElapsed += elapsed / 1000; this.applyMotion(); }
  }
  // Simulation can emit several animation frames between canvas submissions.
  // Keep a muzzle-flash frame until the GPU has actually presented it, then
  // catch up to the newest state. This prevents short flashes vanishing at low FPS.
  requestBatch(frames) {
    if (this.closed || !frames.length) return null;
    const latest = frames[frames.length - 1];
    if (this.awaitingFlash) { this.afterFlash = latest; return null; }
    const flash = [...frames].reverse().find(frame => frame.visible === true &&
      (frame.clip === 'fireHip' || frame.clip === 'fireADS') && (frame.index === 1 || frame.index === 2));
    const id = flash && this.clips[flash.clip]?.frames?.[flash.index - 1];
    if (id) {
      this.awaitingFlash = {id, clip:flash.clip};
      this.afterFlash = latest === flash ? null : latest;
      this.request(flash);
      return flash;
    }
    this.request(latest);
    return latest;
  }
  // Call only after a real canvas submission. A still-loading image stays
  // selected until it uploads, rather than being replaced before it is drawn.
  presented() {
    if (!this.awaitingFlash) return null;
    const {id, clip} = this.awaitingFlash;
    if ((this.uploaded !== id || this.uploadedClip !== clip) && !this.failed.has(id)) return null;
    this.awaitingFlash = null;
    const next = this.afterFlash; this.afterFlash = null;
    if (next) this.request(next);
    return next;
  }
  request(frame) {
    if (this.closed) return;
    this.viewer.set_first_person_settings(frame.visible === true, frame.opacity, frame.key_threshold,
      frame.key_softness, frame.despill, frame.vertical_offset);
    this.presentationSettings = {visible:frame.visible === true,opacity:frame.opacity,vertical_offset:frame.vertical_offset};
    this.motion = {x:Number.isFinite(frame.motion_x) ? frame.motion_x : 0,
      y:Number.isFinite(frame.motion_y) ? frame.motion_y : 0,
      roll:Number.isFinite(frame.motion_roll) ? frame.motion_roll : 0};
    const clip = this.clips[frame.clip];
    if (!clip || !Array.isArray(clip.frames) || !clip.frames.length || !Number.isInteger(frame.index)) return;
    const index = Math.max(0, Math.min(clip.frames.length - 1, frame.index - 1));
    const id = clip.frames[index];
    const direction = frame.direction === -1 ? -1 : frame.direction === 1 ? 1 :
      this.previous?.clip === frame.clip && index < this.previous.index ? -1 : 1;
    if (this.desiredClip !== frame.clip) {
      this.transitionSeconds = Number.isFinite(frame.transition_seconds) ? Math.max(0, Math.min(.3,frame.transition_seconds)) : 0;
      this.desiredClip = frame.clip; this.wrapPending = false;
    } else if (frame.loop && this.previous?.clip === frame.clip && direction > 0 && index < this.previous.index) {
      this.transitionSeconds = Number.isFinite(frame.loop_blend_seconds) ? Math.max(0, Math.min(.3,frame.loop_blend_seconds)) : 0;
      this.wrapPending = this.transitionSeconds > 0;
    }
    this.previous = {clip:frame.clip, index}; this.desired = id; this.desiredIndex = index + 1;
    const bitmap = this.cache.get(id);
    if (bitmap) {
      this.cache.delete(id); this.cache.set(id, bitmap);
      this.display(id, bitmap);
    } else this.enqueue(id, true);
    for (let i=1; i<=8; i++) {
      const next = index + i * direction;
      const wrapped = ((next % clip.frames.length) + clip.frames.length) % clip.frames.length;
      this.enqueue(clip.frames[frame.loop ? wrapped : Math.max(0, Math.min(clip.frames.length-1, next))], false);
    }
    this.applyMotion();
    this.pump();
  }
  close() {
    this.closed = true; this.abort.abort(); this.queue.length = 0;
    this.awaitingFlash = null; this.afterFlash = null;
    for (const bitmap of this.cache.values()) bitmap.close();
    this.cache.clear();
  }
}
