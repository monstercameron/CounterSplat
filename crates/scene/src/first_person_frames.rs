//! Streaming of scene-declared first-person footage frames into the renderer. Lua owns weapon state, frame choice
//! and timing; this resolves frames to assets, schedules decodes (at most four at once, nearest frames first), keeps
//! a bounded cache with the muzzle flashes and the raise animation resident, holds a flash until it was presented,
//! and blends clip changes. Decoding and the GPU are the host's: it starts the loads this asks for, reports their
//! results, and performs the returned `Effect`s.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

const LOAD_LIMIT: usize = 4;
const MAX_CACHE: usize = 48;
const MAX_QUEUE: usize = 96;
const LOOKAHEAD: i64 = 8;

/// What the host must do to the renderer, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect<B> {
    /// `set_first_person_settings(visible, opacity, key_threshold, key_softness, despill, vertical_offset)`.
    Settings([f64; 5], bool),
    /// Upload this frame as the current one.
    Install(B),
    /// Keep the current frame as the blend source; the host answers through `capture_ok`.
    Capture,
    /// `set_first_person_motion(x, y, roll, blend)`.
    Motion([f64; 4]),
    /// Release a decoded frame.
    Close(B),
}

/// One frame request from Lua.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRequest {
    pub raw: Value,
}

impl FrameRequest {
    fn number(&self, key: &str) -> Option<f64> {
        self.raw.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
    }
    fn clip(&self) -> Option<&str> {
        self.raw.get("clip").and_then(Value::as_str)
    }
    fn index(&self) -> Option<i64> {
        self.raw.get("index").and_then(Value::as_i64).filter(|_| self.raw.get("index").and_then(Value::as_f64).is_some_and(|v| v.fract() == 0.0))
    }
    fn visible(&self) -> bool {
        self.raw.get("visible") == Some(&Value::Bool(true))
    }
    fn is_flash(&self) -> bool {
        self.visible() && matches!(self.clip(), Some("fireHip" | "fireADS")) && matches!(self.index(), Some(1 | 2))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Shown {
    clip: String,
    index: i64,
}

/// The scheduler, generic over the host's decoded frame type `B`.
#[derive(Debug)]
pub struct FirstPersonFrames<B> {
    clips: HashMap<String, Vec<String>>,
    cache: Vec<(String, B)>,
    pending: HashSet<String>,
    queue: VecDeque<String>,
    failed: HashSet<String>,
    pub pinned: HashSet<String>,
    closed: bool,
    desired: Option<String>,
    previous: Option<(String, i64)>,
    pub uploaded: Option<String>,
    uploaded_clip: Option<String>,
    uploaded_index: Option<i64>,
    desired_clip: Option<String>,
    desired_index: Option<i64>,
    settings: Option<Value>,
    blend_from: Option<Shown>,
    blend: f64,
    awaiting_flash: Option<(String, String)>,
    after_flash: Option<FrameRequest>,
    transition_seconds: f64,
    blend_elapsed: f64,
    blend_duration: f64,
    last_time: Option<f64>,
    wrap_pending: bool,
    motion: [f64; 3],
}

impl<B: Clone> FirstPersonFrames<B> {
    /// `manifest` maps clip names to `{frames: [asset id, ...]}`.
    pub fn new(manifest: &Value) -> Self {
        let clips: HashMap<String, Vec<String>> = manifest
            .as_object()
            .map(|clips| {
                clips
                    .iter()
                    .map(|(name, clip)| {
                        let frames = clip
                            .get("frames")
                            .and_then(Value::as_array)
                            .map(|f| f.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
                            .unwrap_or_default();
                        (name.clone(), frames)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let frames = |name: &str| clips.get(name).cloned().unwrap_or_default();
        // The first two frames of each muzzle flash stay resident, so a shot shows immediately; so does the raise.
        let mut pinned: HashSet<String> = HashSet::new();
        let mut warm: Vec<String> = Vec::new();
        for clip in ["fireHip", "fireADS"] {
            for id in frames(clip).into_iter().take(2) {
                if pinned.insert(id.clone()) {
                    warm.push(id);
                }
            }
        }
        for id in frames("raising") {
            if pinned.insert(id.clone()) {
                warm.push(id);
            }
        }
        let firing: Vec<String> = ["fireHip", "fireADS"].into_iter().flat_map(&frames).collect();
        let mut this = Self {
            clips,
            cache: Vec::new(),
            pending: HashSet::new(),
            queue: VecDeque::new(),
            failed: HashSet::new(),
            pinned,
            closed: false,
            desired: None,
            previous: None,
            uploaded: None,
            uploaded_clip: None,
            uploaded_index: None,
            desired_clip: None,
            desired_index: None,
            settings: None,
            blend_from: None,
            blend: 1.0,
            awaiting_flash: None,
            after_flash: None,
            transition_seconds: 0.0,
            blend_elapsed: 0.0,
            blend_duration: 0.0,
            last_time: None,
            wrap_pending: false,
            motion: [0.0; 3],
        };
        // Warm the raise and the brief firing takes before they are needed.
        for id in warm.into_iter().chain(firing) {
            this.enqueue(&id, false);
        }
        this
    }

    pub fn set_clip(&mut self, name: &str, frames: Vec<String>) {
        self.clips.insert(name.to_string(), frames);
    }

    fn cached(&self, id: &str) -> bool {
        self.cache.iter().any(|(key, _)| key == id)
    }

    fn enqueue(&mut self, id: &str, priority: bool) {
        if id.is_empty() || self.cached(id) || self.pending.contains(id) || self.failed.contains(id) {
            return;
        }
        if let Some(at) = self.queue.iter().position(|queued| queued == id) {
            if !priority {
                return;
            }
            self.queue.remove(at);
        }
        if priority {
            self.queue.push_front(id.to_string());
        } else {
            self.queue.push_back(id.to_string());
        }
        self.queue.truncate(MAX_QUEUE);
    }

    /// Frames the host should start decoding now (each must be answered by `loaded`).
    pub fn take_loads(&mut self) -> Vec<String> {
        let mut loads = Vec::new();
        while !self.closed && self.pending.len() < LOAD_LIMIT {
            let Some(id) = self.queue.pop_front() else {
                break;
            };
            if self.cached(&id) || self.pending.contains(&id) || self.failed.contains(&id) {
                continue;
            }
            self.pending.insert(id.clone());
            loads.push(id);
        }
        loads
    }

    /// A decode finished. Failed frames are not retried.
    pub fn loaded(&mut self, id: &str, result: Result<B, String>, effects: &mut Vec<Effect<B>>) {
        self.pending.remove(id);
        match result {
            Ok(frame) if self.closed => effects.push(Effect::Close(frame)),
            Ok(frame) => {
                self.cache.retain(|(key, _)| key != id);
                self.cache.push((id.to_string(), frame.clone()));
                self.evict(effects);
                if self.desired.as_deref() == Some(id) && self.cached(id) {
                    self.display(id, frame, effects);
                }
            }
            Err(_) => {
                if !self.closed {
                    self.failed.insert(id.to_string());
                }
            }
        }
    }

    fn evict(&mut self, effects: &mut Vec<Effect<B>>) {
        while self.cache.len() > MAX_CACHE {
            let Some(at) = self
                .cache
                .iter()
                .position(|(key, _)| Some(key.as_str()) != self.desired.as_deref() && !self.pinned.contains(key))
            else {
                break;
            };
            let (_, frame) = self.cache.remove(at);
            effects.push(Effect::Close(frame));
        }
    }

    fn display(&mut self, id: &str, frame: B, effects: &mut Vec<Effect<B>>) {
        if self.uploaded.as_deref() == Some(id) {
            self.uploaded_clip = self.desired_clip.clone();
            self.uploaded_index = self.desired_index;
            self.wrap_pending = false;
            return;
        }
        if self.uploaded.is_some() && (self.uploaded_clip != self.desired_clip || self.wrap_pending) {
            self.blend_duration = self.transition_seconds;
            self.blend_elapsed = 0.0;
            let previous = Shown {
                clip: self.uploaded_clip.clone().unwrap_or_default(),
                index: self.uploaded_index.unwrap_or(0),
            };
            if self.blend_duration > 0.0 {
                // The host captures and reports back; a failed capture cancels the blend (see `capture_ok`).
                effects.push(Effect::Capture);
                self.blend_from = Some(previous);
            } else {
                self.blend_from = None;
            }
        }
        effects.push(Effect::Install(frame));
        self.uploaded = Some(id.to_string());
        self.uploaded_clip = self.desired_clip.clone();
        self.uploaded_index = self.desired_index;
        self.wrap_pending = false;
        self.apply_motion(effects);
    }

    /// The renderer could not keep the previous frame: show the new one without blending.
    pub fn capture_failed(&mut self, effects: &mut Vec<Effect<B>>) {
        self.blend_duration = 0.0;
        self.blend_from = None;
        self.apply_motion(effects);
    }

    fn apply_motion(&mut self, effects: &mut Vec<Effect<B>>) {
        let progress = if self.blend_duration > 0.0 { (self.blend_elapsed / self.blend_duration).min(1.0) } else { 1.0 };
        let smooth = progress * progress * (3.0 - 2.0 * progress);
        self.blend = smooth;
        effects.push(Effect::Motion([self.motion[0], self.motion[1], self.motion[2], smooth]));
        if progress >= 1.0 {
            self.blend_duration = 0.0;
            self.blend_from = None;
        }
    }

    /// What is installed now (not a newer request still decoding), for the HUD.
    pub fn presentation(&self) -> Value {
        let (Some(settings), Some(clip), Some(index)) = (&self.settings, &self.uploaded_clip, self.uploaded_index) else {
            return json!({"visible": false});
        };
        if index == 0 {
            return json!({"visible": false});
        }
        let mut out = settings.clone();
        if let Some(object) = out.as_object_mut() {
            object.insert("clip".into(), clip.clone().into());
            object.insert("index".into(), index.into());
            object.insert("motion_x".into(), self.motion[0].into());
            object.insert("motion_y".into(), self.motion[1].into());
            object.insert("motion_roll".into(), self.motion[2].into());
            object.insert("blend".into(), self.blend.into());
            object.insert(
                "previous".into(),
                self.blend_from.as_ref().map_or(Value::Null, |p| json!({"clip": p.clip, "index": p.index})),
            );
        }
        out
    }

    /// Advances clip blending to `now` (milliseconds).
    pub fn update(&mut self, now: f64, effects: &mut Vec<Effect<B>>) {
        if self.closed || !now.is_finite() {
            return;
        }
        let elapsed = match self.last_time {
            Some(last) if now >= last => (now - last).min(50.0),
            _ => 0.0,
        };
        self.last_time = Some(now);
        if self.blend_duration > 0.0 {
            self.blend_elapsed += elapsed / 1000.0;
            self.apply_motion(effects);
        }
    }

    /// Several simulation frames may pass between canvas submissions. A muzzle-flash frame is held until it was
    /// presented, then the newest state follows. Returns the frame requested now.
    pub fn request_batch(&mut self, frames: Vec<FrameRequest>, effects: &mut Vec<Effect<B>>) -> Option<FrameRequest> {
        if self.closed || frames.is_empty() {
            return None;
        }
        let latest = frames.last().cloned()?;
        if self.awaiting_flash.is_some() {
            self.after_flash = Some(latest);
            return None;
        }
        let flash = frames.iter().rev().find(|frame| frame.is_flash()).cloned();
        let flash_id = flash.as_ref().and_then(|f| {
            let index = usize::try_from(f.index()? - 1).ok()?;
            self.clips.get(f.clip()?)?.get(index).cloned()
        });
        if let (Some(flash), Some(id)) = (flash, flash_id) {
            self.awaiting_flash = Some((id, flash.clip().unwrap_or_default().to_string()));
            self.after_flash = (latest != flash).then_some(latest);
            self.request(&flash, effects);
            return Some(flash);
        }
        self.request(&latest, effects);
        Some(latest)
    }

    /// After a real canvas submission: releases a held flash once it was drawn (or failed), requesting the newest
    /// state. A still-decoding flash stays selected.
    pub fn presented(&mut self, effects: &mut Vec<Effect<B>>) -> Option<FrameRequest> {
        let (id, clip) = self.awaiting_flash.clone()?;
        if (self.uploaded.as_deref() != Some(&id) || self.uploaded_clip.as_deref() != Some(&clip)) && !self.failed.contains(&id) {
            return None;
        }
        self.awaiting_flash = None;
        let next = self.after_flash.take();
        if let Some(next) = &next {
            self.request(next, effects);
        }
        next
    }

    pub fn request(&mut self, frame: &FrameRequest, effects: &mut Vec<Effect<B>>) {
        if self.closed {
            return;
        }
        let number = |key: &str| frame.number(key).unwrap_or(f64::NAN);
        effects.push(Effect::Settings(
            [number("opacity"), number("key_threshold"), number("key_softness"), number("despill"), number("vertical_offset")],
            frame.visible(),
        ));
        self.settings = Some(json!({"visible": frame.visible(), "opacity": frame.raw.get("opacity"),
            "vertical_offset": frame.raw.get("vertical_offset")}));
        self.motion = [
            frame.number("motion_x").unwrap_or(0.0),
            frame.number("motion_y").unwrap_or(0.0),
            frame.number("motion_roll").unwrap_or(0.0),
        ];
        let (Some(clip_name), Some(requested)) = (frame.clip().map(str::to_string), frame.index()) else {
            return;
        };
        let Some(clip) = self.clips.get(&clip_name).filter(|c| !c.is_empty()).cloned() else {
            return;
        };
        let len = clip.len() as i64;
        let index = (requested - 1).clamp(0, len - 1);
        let id = clip[index as usize].clone();
        let direction = match frame.raw.get("direction").and_then(Value::as_f64) {
            Some(d) if d == -1.0 => -1,
            Some(d) if d == 1.0 => 1,
            _ => {
                if self.previous.as_ref().is_some_and(|(c, i)| *c == clip_name && index < *i) { -1 } else { 1 }
            }
        };
        let looping = frame.raw.get("loop").and_then(Value::as_bool).unwrap_or(false);
        if self.desired_clip.as_deref() != Some(&clip_name) {
            self.transition_seconds = frame.number("transition_seconds").map_or(0.0, |s| s.clamp(0.0, 0.3));
            self.desired_clip = Some(clip_name.clone());
            self.wrap_pending = false;
        } else if looping
            && direction > 0
            && self.previous.as_ref().is_some_and(|(c, i)| *c == clip_name && index < *i)
        {
            self.transition_seconds = frame.number("loop_blend_seconds").map_or(0.0, |s| s.clamp(0.0, 0.3));
            self.wrap_pending = self.transition_seconds > 0.0;
        }
        self.previous = Some((clip_name, index));
        self.desired = Some(id.clone());
        self.desired_index = Some(index + 1);
        if let Some(at) = self.cache.iter().position(|(key, _)| *key == id) {
            let entry = self.cache.remove(at);
            let frame = entry.1.clone();
            self.cache.push(entry);
            self.display(&id, frame, effects);
        } else {
            self.enqueue(&id, true);
        }
        for step in 1..=LOOKAHEAD {
            let next = index + step * direction;
            let at = if looping { next.rem_euclid(len) } else { next.clamp(0, len - 1) };
            let next_id = clip[at as usize].clone();
            self.enqueue(&next_id, false);
        }
        self.apply_motion(effects);
    }

    pub fn close(&mut self, effects: &mut Vec<Effect<B>>) {
        self.closed = true;
        self.queue.clear();
        self.awaiting_flash = None;
        self.after_flash = None;
        for (_, frame) in self.cache.drain(..) {
            effects.push(Effect::Close(frame));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Frames = FirstPersonFrames<&'static str>;

    fn frame(clip: &str, index: i64) -> FrameRequest {
        FrameRequest { raw: json!({"clip": clip, "index": index, "visible": true, "opacity": 1, "key_threshold": 0.14,
            "key_softness": 0.28, "despill": 0.025, "vertical_offset": 0.055}) }
    }

    fn installs(effects: &[Effect<&'static str>]) -> Vec<&'static str> {
        effects.iter().filter_map(|e| if let Effect::Install(b) = e { Some(*b) } else { None }).collect()
    }

    fn last_motion(effects: &[Effect<&'static str>]) -> [f64; 4] {
        effects.iter().rev().find_map(|e| if let Effect::Motion(m) = e { Some(*m) } else { None }).unwrap()
    }

    #[test]
    fn stale_frames_caching_and_clip_blends() {
        let mut fx = Vec::new();
        let mut p: Frames = FirstPersonFrames::new(&json!({"raising":{"frames":["a","b","c"]}}));
        let warm = p.take_loads();
        assert_eq!(warm, ["a", "b", "c"]);
        p.request(&frame("raising", 1), &mut fx);
        p.request(&frame("raising", 3), &mut fx);
        assert_eq!(p.presentation()["visible"], false, "undecoded footage has no presented anchor");
        for id in ["a", "c", "b"] {
            p.loaded(id, Ok(id), &mut fx);
        }
        assert_eq!(installs(&fx), ["c"], "late frames must never replace the requested frame");
        assert_eq!(p.presentation()["index"], 3);
        p.request(&frame("raising", 1), &mut fx);
        assert_eq!(installs(&fx), ["c", "a"], "cached frames upload immediately");
        p.set_clip("idle", vec!["c".into()]);
        let mut idle = frame("idle", 1);
        idle.raw["transition_seconds"] = json!(0.1);
        idle.raw["motion_x"] = json!(0.01);
        idle.raw["motion_y"] = json!(-0.02);
        idle.raw["motion_roll"] = json!(0.03);
        let before = fx.len();
        p.request(&idle, &mut fx);
        assert!(fx[before..].contains(&Effect::Capture), "a clip change captures the previous frame");
        assert_eq!(last_motion(&fx)[3], 0.0, "the new clip begins at the old frame");
        assert_eq!(p.presentation()["previous"], json!({"clip":"raising","index":1}));
        assert_eq!(p.presentation()["clip"], "idle");
        p.update(1000.0, &mut fx);
        p.update(1050.0, &mut fx);
        assert!((last_motion(&fx)[3] - 0.5).abs() < 1e-8);
        p.update(1100.0, &mut fx);
        assert_eq!(last_motion(&fx), [0.01, -0.02, 0.03, 1.0], "the clip handoff completes");
        assert_eq!(p.presentation()["previous"], Value::Null);
        // Flashes survive later frames until presented.
        p.set_clip("fireADS", vec!["a".into(), "b".into(), "c".into()]);
        let flash = frame("fireADS", 1);
        let mut third = flash.clone();
        third.raw["index"] = json!(3);
        assert_eq!(p.request_batch(vec![flash.clone(), third, frame("idle", 1)], &mut fx), Some(flash));
        assert_eq!(p.uploaded.as_deref(), Some("a"));
        let mut moved = frame("idle", 1);
        moved.raw["motion_x"] = json!(0.02);
        p.request_batch(vec![moved], &mut fx);
        assert_eq!(p.uploaded.as_deref(), Some("a"), "later animation must not overwrite an unpresented flash");
        assert_eq!(p.presented(&mut fx).unwrap().raw["motion_x"], 0.02);
        assert_eq!(p.uploaded.as_deref(), Some("c"), "the newest state follows the rendered flash");
        let mut closing = Vec::new();
        p.close(&mut closing);
        let mut closed: Vec<&str> = closing.iter().filter_map(|e| if let Effect::Close(b) = e { Some(*b) } else { None }).collect();
        closed.sort_unstable();
        assert_eq!(closed, ["a", "b", "c"]);
    }

    #[test]
    fn flashes_load_first_and_wait_for_decode() {
        let mut fx = Vec::new();
        let mut p: Frames = FirstPersonFrames::new(&json!({"raising":{"frames":["a","b","c"]},
            "fireHip":{"frames":["hip"]},"fireADS":{"frames":["ads"]}}));
        assert_eq!(&p.take_loads()[..2], ["hip", "ads"], "brief firing frames load before the raise");
        assert!(p.pinned.contains("hip") && p.pinned.contains("ads"));
        let delayed_idle = frame("raising", 1);
        p.request_batch(vec![frame("fireADS", 1), delayed_idle.clone()], &mut fx);
        assert_eq!(p.presentation()["visible"], false, "a pending image is not reported as displayed");
        assert_eq!(p.presented(&mut fx), None, "a not-yet-decoded flash is not skipped");
        p.loaded("ads", Ok("ads"), &mut fx);
        assert_eq!(p.uploaded.as_deref(), Some("ads"));
        assert_eq!(p.presented(&mut fx), Some(delayed_idle), "the newest state follows once the flash is drawn");
        assert_eq!(p.presentation()["clip"], "fireADS");
        assert_eq!(p.presentation()["index"], 1);
    }
}
