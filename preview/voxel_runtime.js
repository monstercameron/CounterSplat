// Coordinates per-entity ECS bodies with the separate level-collision worker.
// Lua submits movement intents; this host reconciles each entity's worker result.
export class VoxelRuntime {
  constructor(game, base, isCurrent, worker = new Worker('/pkg/voxel-worker.js', {type:'module'})) {
    this.game = game; this.isCurrent = isCurrent; this.worker = worker;
    const ids = this.call({op:'get_resource', name:'scene.entities'}).value;
    this.level = ids.active_level; this.player = ids.player;
    if (!this.level || !this.player) throw new Error('Voxel level or player entity is missing.');
    this.state = 'loading'; this.resolution = null; this.count = 0;
    this.capture = false; this.visible = false; this.bodies = new Map(); this.requests = new Map();
    this.sequence = 0; this.groundRequests = new Map(); this.debugPending = null; this.debugAt = -Infinity; this.debugPosition = null;
    this.setState('loading');
    worker.onmessage = ({data}) => this.onMessage(data);
    worker.onerror = event => this.fail(event.message || 'Voxel worker failed.');
    worker.postMessage({type:'load', base:base.href});
  }
  call(operation) { return JSON.parse(this.game.ecs_operation('gameplay', JSON.stringify(operation))); }
  get(id, name) { return this.call({op:'get', id, name}).value; }
  set(id, name, value) { this.call({op:'set', id, name, value}); }
  alive(id) { return this.call({op:'is_alive', id}).value === true; }
  setCapture(value) {
    if (this.capture === value) return;
    this.capture = value;
    this.call({op:'set_resource', name:'voxel.capture_movement', value});
  }
  setState(status, message = '') {
    this.state = status;
    if (!this.isCurrent()) return;
    this.set(this.level, 'VoxelColliderState', {status, resolution:this.resolution,
      visible_voxels:this.count, message:String(message).replace(/[\x00-\x1f\x7f]/g, ' ').slice(0, 256)});
  }
  camera() { return this.get(this.player, 'PlayerCamera'); }
  static near(a, b) { return a.every((value, i) => Math.abs(value - b[i]) < 0.001); }
  static total(deltas) { return deltas.reduce((sum, delta) => sum.map((v, i) => v + delta[i]), [0,0,0]); }
  release(id, entry, apply) {
    if (entry.pending) this.requests.delete(entry.pending.id);
    if (apply && this.alive(id)) {
      const pose = this.get(id, entry.component);
      if (pose && (!entry.pending || VoxelRuntime.near(pose.position, entry.pending.start))) {
        const delta = VoxelRuntime.total([...(entry.pending?.deltas || []), ...entry.queue]);
        pose.position = pose.position.map((v, i) => Math.max(-10_000_000, Math.min(10_000_000, v + delta[i])));
        this.set(id, entry.component, pose);
      }
    }
    entry.pending = null; entry.queue = [];
    this.bodies.delete(id);
  }
  releaseAll(apply) { for (const [id, entry] of this.bodies) this.release(id, entry, apply); }
  suspend() {
    if (!this.isCurrent()) return;
    this.setCapture(false); this.releaseAll(false); this.groundRequests.clear();
  }
  stop() {
    this.worker.terminate();
    this.bodies.clear(); this.requests.clear(); this.groundRequests.clear(); this.debugPending = null;
  }
  fail(error) {
    if (!this.isCurrent()) return;
    this.setCapture(false); this.releaseAll(true);
    this.visible = false; this.game.clear_voxel_debug(); this.count = 0;
    this.setState('error', error);
    console.warn('Voxel collision:', error);
  }
  onMessage(data) {
    if (!this.isCurrent()) return;
    try {
      if (data.type === 'ready') {
        this.resolution = data.resolution; this.setState('ready');
        this.call({op:'emit', name:'physics.ready', value:{resolution:data.resolution}});
      } else if (data.type === 'unavailable') {
        this.setCapture(false); this.releaseAll(true); this.setState('unavailable');
      } else if (data.type === 'error') {
        if (data.operation === 'ground') {
          const request = this.groundRequests.get(data.id); this.groundRequests.delete(data.id);
          if (request) {
            // A failed query still completes, so Lua can release its spawn hold.
            this.call({op:'emit', name:'physics.ground_result', value:{entity:request.entity, token:request.token, height:null}});
            this.call({op:'emit', name:'physics.error', value:{entity:request.entity, message:String(data.message).slice(0,256)}});
          }
        } else if (data.operation === 'debug') {
          this.debugPending = null; this.debugAt = performance.now();
          console.warn('Voxel debug view:', data.message);
        } else if (data.operation === 'move') {
          const id = this.requests.get(data.id); this.requests.delete(data.id);
          const entry = this.bodies.get(id);
          if (entry?.pending?.id === data.id) {
            entry.pending = null; entry.queue = [];
            if (this.alive(id)) this.call({op:'emit', name:'physics.motion_error',
              value:{entity:id, message:String(data.message).slice(0,256)}});
          }
        } else this.fail(data.message);
      } else if (data.type === 'ground') {
        const request = this.groundRequests.get(data.id); this.groundRequests.delete(data.id);
        if (request && this.alive(request.entity)) {
          this.call({op:'emit', name:'physics.ground_result', value:{entity:request.entity, token:request.token,
            height:Number.isFinite(data.height) ? data.height : null}});
        }
      } else if (data.type === 'move') {
        const id = this.requests.get(data.id); this.requests.delete(data.id);
        const entry = this.bodies.get(id);
        if (!entry || entry.pending?.id !== data.id || !this.alive(id)) return;
        const request = entry.pending; entry.pending = null;
        const pose = this.get(id, entry.component), body = this.get(id, 'PhysicsBody');
        if (this.capture && body?.enabled && body.collision_enabled && body.radius === request.radius &&
            (body.capsule_half_height ?? 0) === request.halfHeight && body.collision_offset_y === request.offsetY &&
            pose && VoxelRuntime.near(pose.position, request.start)) {
          const position = data.position.slice(); position[1] -= request.offsetY;
          const attempted = VoxelRuntime.total(request.deltas);
          const moved = position.map((value, i) => value - request.start[i]);
          pose.position = position; this.set(id, entry.component, pose);
          this.call({op:'emit', name:'physics.motion_resolved', value:{entity:id, attempted, moved}});
        } else {
          // A teleport, removed body, settings change, or a disabled collider superseded this response.
          entry.queue = [];
        }
        this.pump(id, entry);
      } else if (data.type === 'debug' && this.debugPending === data.id) {
        this.debugPending = null;
        if (!this.visible || this.state !== 'ready') return;
        this.game.set_voxel_debug(data.points, this.resolution);
        this.count = data.points.length / 3; this.setState('ready');
      }
    } catch (error) { this.fail(error); }
  }
  pump(id, entry) {
    if (!this.capture || entry.pending || !entry.queue.length || !this.isCurrent() || !this.alive(id)) return;
    const pose = this.get(id, entry.component), body = this.get(id, 'PhysicsBody');
    if (!pose || !body?.enabled || !body.collision_enabled) { this.release(id, entry, true); return; }
    const start = pose.position;
    const deltas = entry.queue.splice(0, 64);
    const requestId = ++this.sequence;
    entry.pending = {id:requestId, start, deltas, radius:body.radius,
      halfHeight:body.capsule_half_height ?? 0, offsetY:body.collision_offset_y};
    this.requests.set(requestId, id);
    const center = start.slice(); center[1] += body.collision_offset_y;
    this.worker.postMessage({type:'move', id:requestId, position:center, deltas, radius:body.radius,
      halfHeight:body.capsule_half_height ?? 0});
  }
  tick(runtime, now) {
    if (!this.isCurrent()) return;
    const settings = this.get(this.level, 'VoxelCollider');
    const capture = this.state === 'ready' && settings.enabled && settings.physics;
    if (this.capture && !capture) this.releaseAll(true);
    this.setCapture(capture);
    for (const [id, entry] of this.bodies) {
      if (!this.alive(id)) { this.release(id, entry, false); continue; }
      const body = this.get(id, 'PhysicsBody');
      if (!capture || !body?.enabled || !body.collision_enabled) { this.release(id, entry, true); continue; }
      if (entry.pending && (body.radius !== entry.pending.radius ||
          (body.capsule_half_height ?? 0) !== entry.pending.halfHeight || body.collision_offset_y !== entry.pending.offsetY)) {
        this.requests.delete(entry.pending.id);
        entry.queue.unshift(...entry.pending.deltas); entry.pending = null;
      }
    }
    // Ground requests are script-authored; the worker only supplies geometry.
    for (const probe of runtime.drain('physics.ground_request')) {
      if (this.state !== 'ready' || this.groundRequests.size >= 16 || typeof probe.entity !== 'string' ||
          !this.alive(probe.entity) || typeof probe.token !== 'string' || probe.token.length < 1 || probe.token.length > 64 ||
          !Array.isArray(probe.position) || probe.position.length !== 3 || !probe.position.every(Number.isFinite) ||
          !Number.isFinite(probe.radius) || probe.radius < 0 || probe.radius > 1 ||
          !Number.isFinite(probe.max_drop) || probe.max_drop < 0.05 || probe.max_drop > 16) continue;
      const id = ++this.sequence; this.groundRequests.set(id, {entity:probe.entity, token:probe.token});
      this.worker.postMessage({type:'ground', id, position:probe.position, radius:probe.radius, maxDrop:probe.max_drop});
    }
    const grouped = new Map();
    for (const move of runtime.drain('physics.move')) {
      if (!capture || typeof move.entity !== 'string' || !['PlayerCamera','Transform'].includes(move.component) ||
          !Array.isArray(move.delta) || move.delta.length !== 3 || !move.delta.every(Number.isFinite) || !this.alive(move.entity)) continue;
      const body = this.get(move.entity, 'PhysicsBody');
      if (!body?.enabled || !body.collision_enabled) continue;
      let group = grouped.get(move.entity);
      if (!group) { group = {component:move.component, deltas:[]}; grouped.set(move.entity, group); }
      if (group.component === move.component) group.deltas.push(move.delta);
    }
    for (const [id, group] of grouped) {
      const pose = this.get(id, group.component);
      if (!pose) continue;
      // Scripts optimistically moved the pose. Roll it back until collision resolves.
      const total = VoxelRuntime.total(group.deltas);
      pose.position = pose.position.map((value, i) => value - total[i]);
      this.set(id, group.component, pose);
      let entry = this.bodies.get(id);
      if (entry && (entry.component !== group.component || (entry.pending && !VoxelRuntime.near(pose.position, entry.pending.start)))) {
        this.release(id, entry, false); entry = null;
      }
      if (!entry) { entry = {component:group.component, queue:[], pending:null}; this.bodies.set(id, entry); }
      // The collider bounds its substeps. Split fast movement into safe queries and
      // stop overflow rather than bypassing collision or failing every entity.
      const body = this.get(id, 'PhysicsBody');
      const maxDistance = Math.max(0.001, Math.min(this.resolution * 0.5, body.radius * 0.5)) * 256 * 0.9;
      for (const delta of group.deltas) {
        const pieces = Math.max(1, Math.ceil(Math.hypot(...delta) / maxDistance));
        const allowed = Math.min(pieces, Math.max(0, 128 - entry.queue.length));
        for (let n = 0; n < allowed; n++) entry.queue.push(delta.map(value => value / pieces));
      }
      this.pump(id, entry);
    }
    for (const [id, entry] of this.bodies) this.pump(id, entry);
    const visible = this.state === 'ready' && settings.enabled && settings.visible;
    if (this.visible && !visible) {
      this.game.clear_voxel_debug(); this.count = 0; this.debugPending = null; this.setState(this.state);
    }
    if (!this.visible && visible) { this.debugAt = -Infinity; this.debugPosition = null; }
    this.visible = visible;
    if (visible && this.debugPending === null && now - this.debugAt >= 400) {
      const position = this.camera().position;
      const moved = !this.debugPosition || position.some((value, i) => Math.abs(value - this.debugPosition[i]) >= 0.5);
      if (moved) {
        this.debugAt = now; this.debugPosition = position.slice();
        const id = ++this.sequence; this.debugPending = id;
        this.worker.postMessage({type:'debug', id, position, radius:3, limit:6000});
      }
    }
  }
}
