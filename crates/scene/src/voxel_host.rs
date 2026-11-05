//! Host-side coordination of per-entity ECS bodies with a separate level-collision worker. Lua submits movement
//! intents (`physics.move`) and queries (ground, clearance, segment traces); the host rolls back the optimistic
//! moves, sends bounded queries to the worker and reconciles each entity's result. The worker only supplies
//! geometry; scripts decide what results mean.
//!
//! The ECS, worker transport, debug overlay and optional latency profile are behind [`Host`], so the rules are
//! testable without a browser.

use serde_json::{json, Map, Value};

const WORLD: f64 = 10_000_000.0;
/// Most movement substeps queued per entity, and most sent in one request.
const QUEUE_LIMIT: usize = 128;
const BATCH: usize = 64;

/// What the coordinator needs from its host.
pub trait Host {
    /// Applies a script operation (`{op, ...}`) to the gameplay ECS and returns the reply's `value`.
    fn ecs(&mut self, operation: Value) -> Result<Value, String>;
    /// Posts a message to the collision worker.
    fn send(&mut self, message: Value);
    fn set_debug(&mut self, points: &[f32], size: f32) -> Result<(), String>;
    fn clear_debug(&mut self);
    fn warn(&mut self, _message: &str) {}
    /// Opt-in latency profile: records a request; true when the worker should time it.
    fn latency_start(&mut self, _id: u64, _operation: &str, _batch: Option<usize>) -> bool {
        false
    }
    /// Records a reply; the request's start time for [`Host::latency_applied`].
    fn latency_finish(&mut self, _id: u64, _worker_ms: Option<f64>, _failed: bool) -> Option<f64> {
        None
    }
    fn latency_applied(&mut self, _started: Option<f64>) {}
    fn latency_clear(&mut self) {}
}

/// A worker reply. Debug points arrive as a typed array, so they travel beside the JSON fields.
pub struct Reply {
    pub data: Value,
    pub points: Option<Vec<f32>>,
}

type Vec3 = [f64; 3];

fn vec3(value: Option<&Value>) -> Option<Vec3> {
    let values = value?.as_array().filter(|v| v.len() == 3)?;
    let mut out = [0.0; 3];
    for (o, v) in out.iter_mut().zip(values) {
        *o = v.as_f64().filter(|v| v.is_finite())?;
    }
    Some(out)
}

fn bounded_point(value: Option<&Value>) -> Option<Vec3> {
    vec3(value).filter(|p| p.iter().all(|v| v.abs() <= WORLD))
}

fn near(a: Vec3, b: Vec3) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.001)
}

fn total(deltas: &[Vec3]) -> Vec3 {
    deltas.iter().fold([0.0; 3], |sum, d| [sum[0] + d[0], sum[1] + d[1], sum[2] + d[2]])
}

fn token(value: Option<&Value>) -> Option<String> {
    value?.as_str().filter(|t| (1..=64).contains(&t.chars().count())).map(String::from)
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn truthy(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn short(message: &str) -> String {
    message.chars().take(256).collect()
}

#[derive(Debug, Clone)]
struct Pending {
    id: u64,
    start: Vec3,
    deltas: Vec<Vec3>,
    radius: f64,
    half_height: f64,
    offset_y: f64,
}

#[derive(Debug, Clone)]
struct Entry {
    component: String,
    queue: Vec<Vec3>,
    pending: Option<Pending>,
}

#[derive(Debug, Clone)]
struct Probe {
    entity: String,
    token: String,
}

/// The body settings a pending move was sent with.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    radius: f64,
    half_height: f64,
    offset_y: f64,
}

fn shape(body: &Value) -> Shape {
    Shape {
        radius: number(body, "radius").unwrap_or(0.0),
        half_height: number(body, "capsule_half_height").unwrap_or(0.0),
        offset_y: number(body, "collision_offset_y").unwrap_or(0.0),
    }
}

fn collides(body: &Value) -> bool {
    truthy(body, "enabled") && truthy(body, "collision_enabled")
}

/// Per-entity collision coordination for one gameplay scene.
pub struct VoxelRuntime {
    pub level: String,
    pub player: String,
    state: &'static str,
    resolution: Option<f64>,
    count: usize,
    capture: bool,
    visible: bool,
    /// Insertion-ordered, so requests go out in the order entities first moved.
    bodies: Vec<(String, Entry)>,
    requests: Vec<(u64, String)>,
    sequence: u64,
    ground: Vec<(u64, Probe)>,
    clearance: Vec<(u64, Probe)>,
    traces: Vec<(u64, Probe)>,
    debug_pending: Option<u64>,
    debug_at: f64,
    debug_position: Option<Vec3>,
    /// The latest tick's clock (ms), for throttling the debug view after a failed query.
    now: f64,
}

fn take<T>(list: &mut Vec<(u64, T)>, id: Option<u64>) -> Option<T> {
    let at = list.iter().position(|(key, _)| Some(*key) == id)?;
    Some(list.remove(at).1)
}

impl VoxelRuntime {
    /// Reads the scene's level and player entities and asks the worker to load the level's voxels from `base`.
    pub fn new(host: &mut impl Host, base: &str) -> Result<Self, String> {
        let ids = host.ecs(json!({"op": "get_resource", "name": "scene.entities"}))?;
        let id = |key| ids.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let (Some(level), Some(player)) = (id("active_level"), id("player")) else {
            return Err("Voxel level or player entity is missing.".into());
        };
        let mut runtime = Self {
            level,
            player,
            state: "loading",
            resolution: None,
            count: 0,
            capture: false,
            visible: false,
            bodies: Vec::new(),
            requests: Vec::new(),
            sequence: 0,
            ground: Vec::new(),
            clearance: Vec::new(),
            traces: Vec::new(),
            debug_pending: None,
            debug_at: f64::NEG_INFINITY,
            debug_position: None,
            now: 0.0,
        };
        runtime.set_state(host, "loading", "")?;
        host.send(json!({"type": "load", "base": base}));
        Ok(runtime)
    }

    fn next_id(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    fn send(&self, host: &mut impl Host, mut message: Value) {
        let id = message.get("id").and_then(Value::as_u64);
        if let Some(id) = id {
            let operation = message["type"].as_str().unwrap_or_default().to_string();
            let batch = message.get("deltas").and_then(Value::as_array).map(Vec::len);
            if host.latency_start(id, &operation, batch) {
                message["profile"] = true.into();
            }
        }
        host.send(message);
    }

    fn get(host: &mut impl Host, id: &str, name: &str) -> Result<Value, String> {
        host.ecs(json!({"op": "get", "id": id, "name": name}))
    }

    fn set(host: &mut impl Host, id: &str, name: &str, value: Value) -> Result<(), String> {
        host.ecs(json!({"op": "set", "id": id, "name": name, "value": value})).map(drop)
    }

    fn alive(host: &mut impl Host, id: &str) -> Result<bool, String> {
        Ok(host.ecs(json!({"op": "is_alive", "id": id}))?.as_bool() == Some(true))
    }

    fn emit(host: &mut impl Host, name: &str, value: Value) -> Result<(), String> {
        host.ecs(json!({"op": "emit", "name": name, "value": value})).map(drop)
    }

    fn set_capture(&mut self, host: &mut impl Host, value: bool) -> Result<(), String> {
        if self.capture == value {
            return Ok(());
        }
        self.capture = value;
        host.ecs(json!({"op": "set_resource", "name": "voxel.capture_movement", "value": value})).map(drop)
    }

    fn set_state(&mut self, host: &mut impl Host, status: &'static str, message: &str) -> Result<(), String> {
        self.state = status;
        let message: String = message.chars().map(|c| if c.is_control() && (c as u32) < 0x80 { ' ' } else { c }).take(256).collect();
        let value = json!({"status": status, "resolution": self.resolution, "visible_voxels": self.count, "message": message});
        Self::set(host, &self.level, "VoxelColliderState", value)
    }

    fn body_index(&self, id: &str) -> Option<usize> {
        self.bodies.iter().position(|(key, _)| key == id)
    }

    /// Ends an entity's collision handling; with `apply`, its unresolved movement is applied unchecked.
    fn release(&mut self, host: &mut impl Host, index: usize, apply: bool) -> Result<(), String> {
        let (id, entry) = self.bodies.remove(index);
        if let Some(pending) = &entry.pending {
            take(&mut self.requests, Some(pending.id));
        }
        if apply && Self::alive(host, &id)? {
            let mut pose = Self::get(host, &id, &entry.component)?;
            if let Some(position) = vec3(pose.get("position")) {
                if entry.pending.as_ref().is_none_or(|p| near(position, p.start)) {
                    let mut deltas: Vec<Vec3> = entry.pending.map(|p| p.deltas).unwrap_or_default();
                    deltas.extend(entry.queue);
                    let delta = total(&deltas);
                    let moved: Vec<f64> = (0..3).map(|i| (position[i] + delta[i]).clamp(-WORLD, WORLD)).collect();
                    pose["position"] = json!(moved);
                    Self::set(host, &id, &entry.component, pose)?;
                }
            }
        }
        Ok(())
    }

    fn release_all(&mut self, host: &mut impl Host, apply: bool) -> Result<(), String> {
        while !self.bodies.is_empty() {
            self.release(host, 0, apply)?;
        }
        Ok(())
    }

    /// Gameplay paused: stop capturing movement and drop pending work (a paused game applies nothing).
    pub fn suspend(&mut self, host: &mut impl Host) {
        let result = self.set_capture(host, false).and_then(|()| self.release_all(host, false));
        self.ground.clear();
        self.clearance.clear();
        host.latency_clear();
        if let Err(error) = result {
            self.fail(host, &error);
        }
    }

    /// The scene ended; nothing more is applied.
    pub fn stop(&mut self, host: &mut impl Host) {
        host.latency_clear();
        self.bodies.clear();
        self.requests.clear();
        self.ground.clear();
        self.clearance.clear();
        self.traces.clear();
        self.debug_pending = None;
    }

    /// The worker failed: movement continues without collision and every trace completes with an error.
    pub fn fail(&mut self, host: &mut impl Host, error: &str) {
        let _ = self.set_capture(host, false);
        let _ = self.release_all(host, true);
        self.bodies.clear();
        host.latency_clear();
        for (_, request) in std::mem::take(&mut self.traces) {
            let _ = self.trace_result(host, &request, "error", None);
        }
        self.visible = false;
        host.clear_debug();
        self.count = 0;
        let _ = self.set_state(host, "error", error);
        host.warn(&format!("Voxel collision: {error}"));
    }

    fn trace_result(&self, host: &mut impl Host, request: &Probe, status: &str, hit: Option<&[f64]>) -> Result<(), String> {
        if !Self::alive(host, &request.entity)? {
            return Ok(());
        }
        let mut value = json!({"entity": request.entity, "token": request.token, "status": status});
        if let Some(hit) = hit {
            value["fraction"] = hit[0].into();
            value["position"] = json!(hit[1..4]);
            value["normal"] = json!(hit[4..7]);
        }
        Self::emit(host, "physics.trace_result", value)
    }

    /// Applies a worker reply.
    pub fn receive(&mut self, host: &mut impl Host, reply: Reply) {
        let data = &reply.data;
        let id = data.get("id").and_then(Value::as_u64);
        let kind = data.get("type").and_then(Value::as_str).unwrap_or_default();
        let started = id.and_then(|id| host.latency_finish(id, number(data, "worker_ms"), kind == "error"));
        if let Err(error) = self.apply(host, data, id, kind, started, reply.points) {
            self.fail(host, &error);
        }
    }

    fn apply(&mut self, host: &mut impl Host, data: &Value, id: Option<u64>, kind: &str, started: Option<f64>, points: Option<Vec<f32>>) -> Result<(), String> {
        let message = || short(data.get("message").and_then(Value::as_str).unwrap_or("undefined"));
        match kind {
            "ready" => {
                self.resolution = number(data, "resolution");
                self.set_state(host, "ready", "")?;
                Self::emit(host, "physics.ready", json!({"resolution": self.resolution}))?;
            }
            "unavailable" => {
                self.set_capture(host, false)?;
                self.release_all(host, true)?;
                self.set_state(host, "unavailable", "")?;
            }
            "error" => match data.get("operation").and_then(Value::as_str).unwrap_or_default() {
                "ground" => {
                    if let Some(request) = take(&mut self.ground, id) {
                        // A failed query still completes, so Lua can release its spawn hold.
                        Self::emit(host, "physics.ground_result", json!({"entity": request.entity, "token": request.token, "height": null}))?;
                        Self::emit(host, "physics.error", json!({"entity": request.entity, "message": message()}))?;
                    }
                }
                "clearance" => {
                    if let Some(request) = take(&mut self.clearance, id) {
                        if Self::alive(host, &request.entity)? {
                            Self::emit(host, "physics.clearance_result", json!({"entity": request.entity, "token": request.token, "clear": false}))?;
                            Self::emit(host, "physics.error", json!({"entity": request.entity, "message": message()}))?;
                        }
                    }
                }
                "trace" => {
                    if let Some(request) = take(&mut self.traces, id) {
                        self.trace_result(host, &request, "error", None)?;
                    }
                }
                "debug" => {
                    self.debug_pending = None;
                    self.debug_at = self.now;
                    host.warn(&format!("Voxel debug view: {}", message()));
                }
                "move" => {
                    let entity = take(&mut self.requests, id);
                    if let Some(index) = entity.as_deref().and_then(|e| self.body_index(e)) {
                        let (entity, entry) = &mut self.bodies[index];
                        if entry.pending.as_ref().map(|p| p.id) == id {
                            entry.pending = None;
                            entry.queue.clear();
                            let entity = entity.clone();
                            if Self::alive(host, &entity)? {
                                Self::emit(host, "physics.motion_error", json!({"entity": entity, "message": message()}))?;
                            }
                        }
                    }
                }
                _ => self.fail(host, &message()),
            },
            "trace" => {
                if let Some(request) = take(&mut self.traces, id) {
                    let hit: Option<Vec<f64>> = data.get("hit").and_then(Value::as_array).map(|h| h.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect());
                    match hit {
                        None => self.trace_result(host, &request, "error", None)?,
                        Some(hit) if hit.is_empty() => self.trace_result(host, &request, "miss", None)?,
                        Some(hit) if hit.len() == 7 && hit.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&hit[0]) => {
                            self.trace_result(host, &request, "hit", Some(&hit))?
                        }
                        Some(_) => self.trace_result(host, &request, "error", None)?,
                    }
                }
            }
            "clearance" => {
                if let Some(request) = take(&mut self.clearance, id) {
                    if Self::alive(host, &request.entity)? {
                        let clear = data.get("clear").and_then(Value::as_bool) == Some(true);
                        Self::emit(host, "physics.clearance_result", json!({"entity": request.entity, "token": request.token, "clear": clear}))?;
                    }
                }
            }
            "ground" => {
                if let Some(request) = take(&mut self.ground, id) {
                    if Self::alive(host, &request.entity)? {
                        let height = number(data, "height");
                        Self::emit(host, "physics.ground_result", json!({"entity": request.entity, "token": request.token, "height": height}))?;
                    }
                }
            }
            "move" => self.resolve_move(host, data, id, started)?,
            "debug" if self.debug_pending.is_some() && self.debug_pending == id => {
                self.debug_pending = None;
                if !self.visible || self.state != "ready" {
                    return Ok(());
                }
                let points = points.unwrap_or_default();
                host.set_debug(&points, self.resolution.unwrap_or(0.0) as f32)?;
                self.count = points.len() / 3;
                self.set_state(host, "ready", "")?;
            }
            _ => {}
        }
        Ok(())
    }

    fn resolve_move(&mut self, host: &mut impl Host, data: &Value, id: Option<u64>, started: Option<f64>) -> Result<(), String> {
        let Some(entity) = take(&mut self.requests, id) else {
            return Ok(());
        };
        let Some(index) = self.body_index(&entity) else {
            return Ok(());
        };
        if self.bodies[index].1.pending.as_ref().map(|p| p.id) != id || !Self::alive(host, &entity)? {
            return Ok(());
        }
        let request = self.bodies[index].1.pending.take().expect("pending checked above");
        let component = self.bodies[index].1.component.clone();
        let mut pose = Self::get(host, &entity, &component)?;
        let body = Self::get(host, &entity, "PhysicsBody")?;
        let current = vec3(pose.get("position"));
        let resolved = vec3(data.get("position"));
        let same_shape = shape(&body) == Shape { radius: request.radius, half_height: request.half_height, offset_y: request.offset_y };
        match (current, resolved) {
            (Some(current), Some(mut position)) if self.capture && collides(&body) && same_shape && near(current, request.start) => {
                position[1] -= request.offset_y;
                let attempted = total(&request.deltas);
                let moved: Vec<f64> = (0..3).map(|i| position[i] - request.start[i]).collect();
                pose["position"] = json!(position);
                Self::set(host, &entity, &component, pose)?;
                host.latency_applied(started);
                Self::emit(host, "physics.motion_resolved", json!({"entity": entity, "attempted": attempted, "moved": moved}))?;
            }
            // A teleport, removed body, settings change, or a disabled collider superseded this response.
            _ => self.bodies[index].1.queue.clear(),
        }
        self.pump(host, &entity)
    }

    /// Sends the entity's next batch of queued substeps when nothing is in flight for it.
    fn pump(&mut self, host: &mut impl Host, entity: &str) -> Result<(), String> {
        let Some(index) = self.body_index(entity) else {
            return Ok(());
        };
        let entry = &self.bodies[index].1;
        if !self.capture || entry.pending.is_some() || entry.queue.is_empty() || !Self::alive(host, entity)? {
            return Ok(());
        }
        let component = entry.component.clone();
        let pose = Self::get(host, entity, &component)?;
        let body = Self::get(host, entity, "PhysicsBody")?;
        let Some(start) = vec3(pose.get("position")).filter(|_| collides(&body)) else {
            return self.release(host, index, true);
        };
        let id = self.next_id();
        let entry = &mut self.bodies[index].1;
        let deltas: Vec<Vec3> = entry.queue.drain(..entry.queue.len().min(BATCH)).collect();
        let Shape { radius, half_height, offset_y } = shape(&body);
        entry.pending = Some(Pending { id, start, deltas: deltas.clone(), radius, half_height, offset_y });
        self.requests.push((id, entity.to_string()));
        let centre = [start[0], start[1] + offset_y, start[2]];
        self.send(host, json!({"type": "move", "id": id, "position": centre, "deltas": deltas, "radius": radius, "halfHeight": half_height}));
        Ok(())
    }

    fn drain(host: &mut impl Host, name: &str) -> Result<Vec<Value>, String> {
        Ok(match host.ecs(json!({"op": "drain", "name": name}))? {
            Value::Array(events) => events,
            _ => Vec::new(),
        })
    }

    /// Runs after the frame's scripts: routes their queries and movement to the worker and keeps the debug view.
    pub fn tick(&mut self, host: &mut impl Host, now: f64) {
        if let Err(error) = self.try_tick(host, now) {
            self.fail(host, &error);
        }
    }

    fn try_tick(&mut self, host: &mut impl Host, now: f64) -> Result<(), String> {
        self.now = now;
        let settings = Self::get(host, &self.level.clone(), "VoxelCollider")?;
        let ready = self.state == "ready";
        let capture = ready && truthy(&settings, "enabled") && truthy(&settings, "physics");
        if self.capture && !capture {
            self.release_all(host, true)?;
        }
        self.set_capture(host, capture)?;
        let mut index = 0;
        while index < self.bodies.len() {
            let id = self.bodies[index].0.clone();
            if !Self::alive(host, &id)? {
                self.release(host, index, false)?;
                continue;
            }
            let body = Self::get(host, &id, "PhysicsBody")?;
            if !capture || !collides(&body) {
                self.release(host, index, true)?;
                continue;
            }
            let entry = &mut self.bodies[index].1;
            if let Some(pending) = entry.pending.take() {
                if shape(&body) != (Shape { radius: pending.radius, half_height: pending.half_height, offset_y: pending.offset_y }) {
                    take(&mut self.requests, Some(pending.id));
                    entry.queue.splice(0..0, pending.deltas);
                } else {
                    entry.pending = Some(pending);
                }
            }
            index += 1;
        }
        // Each request gets an explicit completion, even if collision is disabled or busy. The Lua system owns
        // projectile time, responses, and any resulting damage.
        for trace in Self::drain(host, "physics.trace_request")? {
            let (Some(entity), Some(token)) = (trace.get("entity").and_then(Value::as_str), token(trace.get("token"))) else {
                continue;
            };
            if !Self::alive(host, entity)? {
                continue;
            }
            let request = Probe { entity: entity.into(), token };
            let (Some(start), Some(finish)) = (bounded_point(trace.get("start")), bounded_point(trace.get("finish"))) else {
                self.trace_result(host, &request, "error", None)?;
                continue;
            };
            if !capture {
                self.trace_result(host, &request, "unavailable", None)?;
                continue;
            }
            if self.traces.len() >= 128 {
                self.trace_result(host, &request, "error", None)?;
                continue;
            }
            let id = self.next_id();
            self.traces.push((id, request));
            self.send(host, json!({"type": "trace", "id": id, "start": start, "end": finish}));
        }
        // Ground requests are script-authored; the worker only supplies geometry.
        for probe in Self::drain(host, "physics.ground_request")? {
            let (Some(entity), Some(token)) = (probe.get("entity").and_then(Value::as_str), token(probe.get("token"))) else {
                continue;
            };
            let radius = number(&probe, "radius").filter(|r| (0.0..=1.0).contains(r));
            let max_drop = number(&probe, "max_drop").filter(|d| (0.05..=16.0).contains(d));
            let (Some(position), Some(radius), Some(max_drop)) = (vec3(probe.get("position")), radius, max_drop) else {
                continue;
            };
            if self.state != "ready" || self.ground.len() >= 16 || !Self::alive(host, entity)? {
                continue;
            }
            let id = self.next_id();
            self.ground.push((id, Probe { entity: entity.into(), token }));
            self.send(host, json!({"type": "ground", "id": id, "position": position, "radius": radius, "maxDrop": max_drop}));
        }
        // The script decides what volume to check and what the result means.
        for probe in Self::drain(host, "physics.clearance_request")? {
            let (Some(entity), Some(token)) = (probe.get("entity").and_then(Value::as_str), token(probe.get("token"))) else {
                continue;
            };
            let radius = number(&probe, "radius").filter(|r| (0.05..=1.0).contains(r));
            let half_height = number(&probe, "half_height").filter(|h| (0.0..=4.0).contains(h));
            let (Some(centre), Some(radius), Some(half_height)) = (bounded_point(probe.get("center")), radius, half_height) else {
                continue;
            };
            if !capture || self.clearance.len() >= 16 || !Self::alive(host, entity)? {
                continue;
            }
            let id = self.next_id();
            self.clearance.push((id, Probe { entity: entity.into(), token }));
            self.send(host, json!({"type": "clearance", "id": id, "center": centre, "radius": radius, "halfHeight": half_height}));
        }
        let mut grouped: Vec<(String, String, Vec<Vec3>)> = Vec::new();
        for movement in Self::drain(host, "physics.move")? {
            let entity = movement.get("entity").and_then(Value::as_str);
            let component = movement.get("component").and_then(Value::as_str).filter(|c| matches!(*c, "PlayerCamera" | "Transform"));
            let (true, Some(entity), Some(component), Some(delta)) = (capture, entity, component, vec3(movement.get("delta"))) else {
                continue;
            };
            if !Self::alive(host, entity)? || !collides(&Self::get(host, entity, "PhysicsBody")?) {
                continue;
            }
            match grouped.iter_mut().find(|(id, _, _)| id == entity) {
                Some((_, existing, deltas)) => {
                    if existing == component {
                        deltas.push(delta);
                    }
                }
                None => grouped.push((entity.into(), component.into(), vec![delta])),
            }
        }
        for (id, component, deltas) in grouped {
            let mut pose = Self::get(host, &id, &component)?;
            let Some(position) = vec3(pose.get("position")) else {
                continue;
            };
            // Scripts optimistically moved the pose. Roll it back until collision resolves.
            let sum = total(&deltas);
            let rolled = [position[0] - sum[0], position[1] - sum[1], position[2] - sum[2]];
            pose["position"] = json!(rolled);
            Self::set(host, &id, &component, pose)?;
            if let Some(index) = self.body_index(&id) {
                let entry = &self.bodies[index].1;
                if entry.component != component || entry.pending.as_ref().is_some_and(|p| !near(rolled, p.start)) {
                    self.release(host, index, false)?;
                }
            }
            if self.body_index(&id).is_none() {
                self.bodies.push((id.clone(), Entry { component, queue: Vec::new(), pending: None }));
            }
            // The collider bounds its substeps. Split fast movement into safe queries and stop overflow rather
            // than bypassing collision or failing every entity.
            let body = Self::get(host, &id, "PhysicsBody")?;
            let resolution = self.resolution.unwrap_or(f64::NAN);
            let max_distance = (resolution * 0.5).min(shape(&body).radius * 0.5).max(0.001) * 256.0 * 0.9;
            let index = self.body_index(&id).expect("inserted above");
            let queue = &mut self.bodies[index].1.queue;
            for delta in deltas {
                let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                let pieces = ((length / max_distance).ceil() as usize).max(1);
                let allowed = pieces.min(QUEUE_LIMIT.saturating_sub(queue.len()));
                let piece = delta.map(|v| v / pieces as f64);
                queue.extend(std::iter::repeat_n(piece, allowed));
            }
            self.pump(host, &id)?;
        }
        let ids: Vec<String> = self.bodies.iter().map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.pump(host, &id)?;
        }
        let visible = ready && truthy(&settings, "enabled") && truthy(&settings, "visible");
        if self.visible && !visible {
            host.clear_debug();
            self.count = 0;
            self.debug_pending = None;
            let state = self.state;
            self.set_state(host, state, "")?;
        }
        if !self.visible && visible {
            self.debug_at = f64::NEG_INFINITY;
            self.debug_position = None;
        }
        self.visible = visible;
        if visible && self.debug_pending.is_none() && now - self.debug_at >= 400.0 {
            let camera = Self::get(host, &self.player.clone(), "PlayerCamera")?;
            if let Some(position) = vec3(camera.get("position")) {
                let moved = self.debug_position.is_none_or(|last| position.iter().zip(last).any(|(a, b)| (a - b).abs() >= 0.5));
                if moved {
                    self.debug_at = now;
                    self.debug_position = Some(position);
                    let id = self.next_id();
                    self.debug_pending = Some(id);
                    self.send(host, json!({"type": "debug", "id": id, "position": position, "radius": 3, "limit": 6000}));
                }
            }
        }
        Ok(())
    }
}

/// `{value}` of an ECS reply, for hosts that speak the JSON script protocol.
pub fn reply_value(reply: &str) -> Result<Value, String> {
    let mut reply: Map<String, Value> = serde_json::from_str(reply).map_err(|e| e.to_string())?;
    Ok(reply.remove("value").unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        values: Map<String, Value>,
        resources: Map<String, Value>,
        events: Vec<Value>,
        moves: Vec<Value>,
        sent: Vec<Value>,
        debug: Option<Vec<f32>>,
        drains: Map<String, Value>,
        calls: Vec<String>,
        profile: bool,
    }

    impl Host for Fake {
        fn ecs(&mut self, op: Value) -> Result<Value, String> {
            let s = |k: &str| op[k].as_str().unwrap_or_default().to_string();
            Ok(match op["op"].as_str().unwrap() {
                "get_resource" => self.resources.get(&s("name")).cloned().unwrap_or(Value::Null),
                "set_resource" => {
                    self.resources.insert(s("name"), op["value"].clone());
                    Value::Null
                }
                "is_alive" => self.values.contains_key(&s("id")).into(),
                "get" => self.values.get(&s("id")).and_then(|v| v.get(s("name"))).cloned().unwrap_or(Value::Null),
                "set" => {
                    self.values.get_mut(&s("id")).unwrap()[s("name")] = op["value"].clone();
                    Value::Null
                }
                "emit" => {
                    self.events.push(op.clone());
                    Value::Null
                }
                "drain" => self.drains.remove(&s("name")).unwrap_or(json!([])),
                other => panic!("unexpected {other}"),
            })
        }
        fn send(&mut self, message: Value) {
            if message["type"] == "move" {
                self.moves.push(message.clone());
            }
            self.sent.push(message);
        }
        fn set_debug(&mut self, points: &[f32], _size: f32) -> Result<(), String> {
            self.debug = Some(points.to_vec());
            Ok(())
        }
        fn clear_debug(&mut self) {
            self.debug = Some(Vec::new());
        }
        fn latency_start(&mut self, _id: u64, operation: &str, batch: Option<usize>) -> bool {
            self.calls.push(format!("start {operation} {batch:?}"));
            self.profile
        }
        fn latency_finish(&mut self, _id: u64, worker_ms: Option<f64>, _failed: bool) -> Option<f64> {
            self.calls.push(format!("finish {worker_ms:?}"));
            Some(10.0)
        }
        fn latency_applied(&mut self, started: Option<f64>) {
            self.calls.push(format!("applied {started:?}"));
        }
    }

    fn body(radius: f64) -> Value {
        json!({"enabled":true,"collision_enabled":true,"gravity_enabled":false,"radius":radius,"collision_offset_y":0,"gravity":18,"velocity":[0,0,0],"grounded":false})
    }

    fn game() -> Fake {
        let mut player_body = body(0.2);
        player_body["collision_offset_y"] = json!(-1.4);
        player_body["capsule_half_height"] = json!(0.6);
        let mut fake = Fake::default();
        fake.values.insert("player".into(), json!({"PlayerCamera":{"position":[0,0,0],"yaw":0,"pitch":0,"fov_degrees":60},"PhysicsBody":player_body}));
        fake.values.insert("prop".into(), json!({"Transform":{"position":[10,0,0]},"PhysicsBody":body(0.4)}));
        fake.values.insert("level".into(), json!({"VoxelCollider":{"enabled":true,"visible":false,"physics":true,"player_radius":0.2},"VoxelColliderState":{}}));
        fake.resources.insert("scene.entities".into(), json!({"active_level":"level","player":"player"}));
        fake
    }

    fn reply(data: Value) -> Reply {
        Reply { data, points: None }
    }

    fn queue(fake: &mut Fake, channel: &str, event: Value) {
        fake.drains.entry(channel.to_string()).or_insert(json!([])).as_array_mut().unwrap().push(event);
    }

    fn last(fake: &Fake, kind: &str) -> Value {
        fake.sent.iter().rev().find(|m| m["type"] == kind).cloned().unwrap()
    }

    fn position(fake: &Fake, id: &str, component: &str) -> Value {
        fake.values[id][component]["position"].clone()
    }

    #[test]
    fn queries_route_as_data_and_failures_complete() {
        let mut g = game();
        let mut rt = VoxelRuntime::new(&mut g, "http://localhost/level/").unwrap();
        assert_eq!(g.sent[0], json!({"type":"load","base":"http://localhost/level/"}));
        rt.receive(&mut g, reply(json!({"type":"ready","resolution":0.05})));
        rt.tick(&mut g, 0.0);
        assert_eq!(g.resources["voxel.capture_movement"], true);
        assert_eq!(g.events.last().unwrap()["name"], "physics.ready");

        queue(&mut g, "physics.ground_request", json!({"entity":"player","token":"spawn","position":[0,0,0],"radius":0.1,"max_drop":4}));
        rt.tick(&mut g, 1.0);
        let ground = last(&g, "ground");
        rt.receive(&mut g, reply(json!({"type":"ground","id":ground["id"],"height":-1.5})));
        assert_eq!(g.events.last().unwrap()["value"]["height"], -1.5);
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0, 0, 0]));

        queue(&mut g, "physics.ground_request", json!({"entity":"player","token":"failed","position":[0,0,0],"radius":0.1,"max_drop":4}));
        rt.tick(&mut g, 2.0);
        let failed = last(&g, "ground");
        rt.receive(&mut g, reply(json!({"type":"error","operation":"ground","id":failed["id"],"message":"bad ground"})));
        let n = g.events.len();
        assert_eq!(g.events[n - 2]["name"], "physics.ground_result");
        assert!(g.events[n - 2]["value"]["height"].is_null());
        assert_eq!(g.events[n - 1]["name"], "physics.error");

        queue(&mut g, "physics.clearance_request", json!({"entity":"player","token":"stand-1","center":[0,1,0],"radius":0.2,"half_height":0.4}));
        rt.tick(&mut g, 3.0);
        let clearance = last(&g, "clearance");
        assert_eq!(clearance["center"], json!([0.0, 1.0, 0.0]));
        assert_eq!(clearance["halfHeight"], 0.4);
        rt.receive(&mut g, reply(json!({"type":"clearance","id":clearance["id"],"clear":true})));
        assert_eq!(g.events.last().unwrap()["value"], json!({"entity":"player","token":"stand-1","clear":true}));
        queue(&mut g, "physics.clearance_request", json!({"entity":"player","token":"stand-2","center":[0,1,0],"radius":0.2,"half_height":0.4}));
        rt.tick(&mut g, 4.0);
        let failed = last(&g, "clearance");
        rt.receive(&mut g, reply(json!({"type":"error","operation":"clearance","id":failed["id"],"message":"query failed"})));
        let n = g.events.len();
        assert_eq!(g.events[n - 2]["value"]["clear"], false);

        // Segment traces return the exact worker result, and each token completes only once.
        queue(&mut g, "physics.trace_request", json!({"entity":"player","token":"shot-1","start":[0,0,0],"finish":[10,0,0]}));
        rt.tick(&mut g, 700.0);
        let ray = last(&g, "trace");
        assert_eq!(ray["end"], json!([10.0, 0.0, 0.0]));
        rt.receive(&mut g, reply(json!({"type":"trace","id":ray["id"],"hit":[0.2,2,0,0,-1,0,0]})));
        assert_eq!(g.events.last().unwrap()["value"], json!({"entity":"player","token":"shot-1","status":"hit","fraction":0.2,"position":[2.0,0.0,0.0],"normal":[-1.0,0.0,0.0]}));
        let n = g.events.len();
        rt.receive(&mut g, reply(json!({"type":"trace","id":ray["id"],"hit":[]})));
        assert_eq!(g.events.len(), n);
        queue(&mut g, "physics.trace_request", json!({"entity":"player","token":"malformed","start":[0,0,0],"finish":[1,0,0]}));
        rt.tick(&mut g, 701.0);
        let malformed = last(&g, "trace");
        rt.receive(&mut g, reply(json!({"type":"trace","id":malformed["id"],"hit":null})));
        assert_eq!(g.events.last().unwrap()["value"]["status"], "error");
        g.values["level"]["VoxelCollider"]["enabled"] = json!(false);
        queue(&mut g, "physics.trace_request", json!({"entity":"player","token":"shot-3","start":[0,0,0],"finish":[1,0,0]}));
        rt.tick(&mut g, 1000.0);
        assert_eq!(g.events.last().unwrap()["value"]["status"], "unavailable");
        assert_eq!(g.resources["voxel.capture_movement"], false);
    }

    #[test]
    fn entities_move_independently_and_reconcile() {
        let mut g = game();
        let mut rt = VoxelRuntime::new(&mut g, "base").unwrap();
        rt.receive(&mut g, reply(json!({"type":"ready","resolution":0.05})));
        rt.tick(&mut g, 0.0);
        g.values["player"]["PlayerCamera"]["position"] = json!([1, 0, 0]);
        g.values["prop"]["Transform"]["position"] = json!([10, -1, 0]);
        g.values["prop"]["PhysicsBody"]["velocity"] = json!([0, -2, 0]);
        queue(&mut g, "physics.move", json!({"entity":"player","component":"PlayerCamera","delta":[1,0,0]}));
        queue(&mut g, "physics.move", json!({"entity":"prop","component":"Transform","delta":[0,-1,0]}));
        rt.tick(&mut g, 16.0);
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0.0, 0.0, 0.0]));
        assert_eq!(position(&g, "prop", "Transform"), json!([10.0, 0.0, 0.0]));
        let moves = g.moves.clone();
        assert_eq!(moves.len(), 2);
        assert_eq!((moves[0]["radius"].as_f64(), moves[1]["radius"].as_f64()), (Some(0.2), Some(0.4)));
        assert_eq!((moves[0]["halfHeight"].as_f64(), moves[1]["halfHeight"].as_f64()), (Some(0.6), Some(0.0)));
        assert_eq!(moves[0]["position"], json!([0.0, -1.4, 0.0]));
        rt.receive(&mut g, reply(json!({"type":"move","id":moves[1]["id"],"position":[10,-0.25,0]})));
        assert_eq!(g.values["prop"]["PhysicsBody"]["velocity"][1], -2);
        assert_eq!(g.events.last().unwrap()["name"], "physics.motion_resolved");
        assert_eq!(g.events.last().unwrap()["value"]["moved"], json!([0.0, -0.25, 0.0]));
        rt.receive(&mut g, reply(json!({"type":"move","id":moves[0]["id"],"position":[0.5,-1.4,0]})));
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0.5, 0.0, 0.0]));

        // A shape edit cancels the stale response and retries with the new settings.
        g.values["player"]["PlayerCamera"]["position"] = json!([1.5, 0, 0]);
        queue(&mut g, "physics.move", json!({"entity":"player","component":"PlayerCamera","delta":[1,0,0]}));
        rt.tick(&mut g, 32.0);
        let stale = g.moves.last().unwrap().clone();
        g.values["player"]["PhysicsBody"]["radius"] = json!(0.3);
        rt.tick(&mut g, 48.0);
        let retry = g.moves.last().unwrap().clone();
        assert_ne!(stale["id"], retry["id"]);
        assert_eq!(retry["radius"], 0.3);
        g.values["player"]["PhysicsBody"]["capsule_half_height"] = json!(0.75);
        rt.tick(&mut g, 49.0);
        let again = g.moves.last().unwrap().clone();
        assert_ne!(again["id"], retry["id"]);
        assert_eq!(again["halfHeight"], 0.75);
        rt.receive(&mut g, reply(json!({"type":"move","id":stale["id"],"position":[1.5,-1.4,0]})));
        rt.receive(&mut g, reply(json!({"type":"move","id":retry["id"],"position":[1.5,-1.4,0]})));
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0.5, 0.0, 0.0]));
        rt.receive(&mut g, reply(json!({"type":"move","id":again["id"],"position":[0.9,-1.4,0]})));
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0.9, 0.0, 0.0]));

        // Disabling collision on one entity releases its pending movement without affecting the other.
        g.values["prop"]["Transform"]["position"] = json!([11, -0.25, 0]);
        queue(&mut g, "physics.move", json!({"entity":"prop","component":"Transform","delta":[1,0,0]}));
        rt.tick(&mut g, 64.0);
        let old = g.moves.last().unwrap().clone();
        g.values["prop"]["PhysicsBody"]["collision_enabled"] = json!(false);
        rt.tick(&mut g, 80.0);
        assert_eq!(position(&g, "prop", "Transform"), json!([11.0, -0.25, 0.0]));
        rt.receive(&mut g, reply(json!({"type":"move","id":old["id"],"position":[10.2,-0.25,0]})));
        assert_eq!(position(&g, "prop", "Transform"), json!([11.0, -0.25, 0.0]));

        // A fast movement is split into safe subqueries, and a failure stays local to that entity.
        g.values["player"]["PlayerCamera"]["position"] = json!([20.9, 0, 0]);
        queue(&mut g, "physics.move", json!({"entity":"player","component":"PlayerCamera","delta":[20,0,0]}));
        rt.tick(&mut g, 96.0);
        let fast = g.moves.last().unwrap().clone();
        let deltas = fast["deltas"].as_array().unwrap();
        assert!(deltas.len() > 1 && deltas.iter().all(|d| d[0].as_f64().unwrap().abs() <= 5.76 + 1e-9));
        rt.receive(&mut g, reply(json!({"type":"error","operation":"move","id":fast["id"],"message":"movement failed"})));
        assert_eq!(g.events.last().unwrap()["name"], "physics.motion_error");
        assert_eq!(g.values["level"]["VoxelColliderState"]["status"], "ready");

        g.values["level"]["VoxelCollider"]["visible"] = json!(true);
        rt.tick(&mut g, 500.0);
        let debug = last(&g, "debug");
        rt.receive(&mut g, Reply { data: json!({"type":"debug","id":debug["id"]}), points: Some(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]) });
        assert_eq!(g.values["level"]["VoxelColliderState"]["visible_voxels"], 2);
        g.values["level"]["VoxelCollider"]["visible"] = json!(false);
        rt.tick(&mut g, 900.0);
        assert_eq!(g.debug, Some(Vec::new()));
    }

    #[test]
    fn latency_profile_sees_request_and_application() {
        let mut g = game();
        g.profile = true;
        let mut rt = VoxelRuntime::new(&mut g, "base").unwrap();
        rt.receive(&mut g, reply(json!({"type":"ready","resolution":0.05})));
        rt.tick(&mut g, 10.0);
        g.values["player"]["PlayerCamera"]["position"] = json!([1, 0, 0]);
        queue(&mut g, "physics.move", json!({"entity":"player","component":"PlayerCamera","delta":[1,0,0]}));
        rt.tick(&mut g, 10.0);
        let request = g.moves.last().unwrap().clone();
        assert_eq!(request["profile"], true);
        rt.receive(&mut g, reply(json!({"type":"move","id":request["id"],"position":[0.5,-1.4,0],"worker_ms":0.75})));
        assert_eq!(position(&g, "player", "PlayerCamera"), json!([0.5, 0.0, 0.0]));
        assert_eq!(g.calls, ["start move Some(1)", "finish Some(0.75)", "applied Some(10.0)"]);
    }

    #[test]
    fn missing_entities_and_worker_failure() {
        let mut g = game();
        g.resources.insert("scene.entities".into(), json!({"player":"player"}));
        assert!(VoxelRuntime::new(&mut g, "base").is_err());
        let mut g = game();
        let mut rt = VoxelRuntime::new(&mut g, "base").unwrap();
        rt.receive(&mut g, reply(json!({"type":"ready","resolution":0.05})));
        rt.tick(&mut g, 0.0);
        queue(&mut g, "physics.trace_request", json!({"entity":"player","token":"t","start":[0,0,0],"finish":[1,0,0]}));
        rt.tick(&mut g, 1.0);
        rt.receive(&mut g, reply(json!({"type":"error","operation":"load","message":"lost\u{1}"})));
        assert_eq!(g.events.last().unwrap()["value"]["status"], "error", "pending traces complete");
        assert_eq!(g.values["level"]["VoxelColliderState"]["status"], "error");
        assert_eq!(g.values["level"]["VoxelColliderState"]["message"], "lost ");
        assert_eq!(g.resources["voxel.capture_movement"], false);
    }
}
