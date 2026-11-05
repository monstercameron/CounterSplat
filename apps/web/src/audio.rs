//! Scene audio in the browser: decodes the scene's preloaded sound clips and mixes the voices Lua requests through Web
//! Audio. The mixing rules are `countersplat_scene::audio_mix`; this never fetches from events.

use countersplat_scene::audio_mix::{self, Layer, MAX_VOICES};
use js_sys::{Reflect, Uint8Array, JSON};
use std::{cell::RefCell, collections::{BTreeMap, VecDeque}, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode, GainNode};

fn to_json(value: &JsValue) -> serde_json::Value {
    if value.is_undefined() {
        return serde_json::Value::Null;
    }
    JSON::stringify(value)
        .ok()
        .and_then(|text| text.as_string())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// The non-deprecated start/stop/onended live on the base interface.
fn scheduled(source: &AudioBufferSourceNode) -> &web_sys::AudioScheduledSourceNode {
    source.as_ref()
}

fn warn(message: String) {
    web_sys::console::warn_1(&message.into());
}

struct Voice {
    source: AudioBufferSourceNode,
    nodes: Vec<AudioNode>,
    id: u64,
}

#[derive(Default)]
struct State {
    context: Option<AudioContext>,
    master: Option<GainNode>,
    encoded: BTreeMap<String, Uint8Array>,
    clips: BTreeMap<String, AudioBuffer>,
    decoding: std::collections::BTreeSet<String>,
    layers: BTreeMap<String, (Layer, Option<GainNode>)>,
    voices: VecDeque<Voice>,
    next_voice: u64,
    generation: u64,
    played: u64,
    on_ended: Option<Closure<dyn FnMut(web_sys::Event)>>,
}

impl State {
    fn apply_layers(&mut self) {
        let (Some(context), Some(master)) = (&self.context, &self.master) else {
            return;
        };
        let gains = audio_mix::layer_gains(self.layers.values().map(|(layer, _)| layer));
        for ((_, node), gain) in self.layers.values_mut().zip(gains) {
            if node.is_none() {
                if let Ok(gain_node) = context.create_gain() {
                    let _ = gain_node.connect_with_audio_node(master);
                    *node = Some(gain_node);
                }
            }
            if let Some(node) = node {
                node.gain().set_value(gain as f32);
            }
        }
    }

    fn dispose(&mut self, id: u64) {
        if let Some(at) = self.voices.iter().position(|voice| voice.id == id) {
            if let Some(voice) = self.voices.remove(at) {
                scheduled(&voice.source).set_onended(None);
                for node in &voice.nodes {
                    node.disconnect().ok();
                }
            }
        }
    }

    fn stop_voices(&mut self) {
        while let Some(voice) = self.voices.pop_front() {
            let _ = scheduled(&voice.source).stop();
            scheduled(&voice.source).set_onended(None);
            for node in &voice.nodes {
                node.disconnect().ok();
            }
        }
    }
}

/// Scene sound playback. `unlock` must be called synchronously from a user gesture (autoplay policy).
#[wasm_bindgen]
#[derive(Clone, Default)]
pub struct SceneAudio(Rc<RefCell<State>>);

fn decode_all(state: &Rc<RefCell<State>>) {
    let (context, generation, pending) = {
        let s = state.borrow();
        let Some(context) = s.context.clone() else {
            return;
        };
        let pending: Vec<(String, Uint8Array)> = s
            .encoded
            .iter()
            .filter(|(id, _)| !s.clips.contains_key(*id) && !s.decoding.contains(*id))
            .map(|(id, bytes)| (id.clone(), bytes.clone()))
            .collect();
        (context, s.generation, pending)
    };
    for (id, bytes) in pending {
        state.borrow_mut().decoding.insert(id.clone());
        let state = state.clone();
        let context = context.clone();
        spawn_local(async move {
            // decodeAudioData detaches its input, so it gets a copy.
            let buffer = bytes.slice(0, bytes.length()).buffer();
            let result = match context.decode_audio_data(&buffer) {
                Ok(promise) => JsFuture::from(promise).await,
                Err(error) => Err(error),
            };
            let mut s = state.borrow_mut();
            if s.generation != generation {
                return;
            }
            s.decoding.remove(&id);
            match result {
                Ok(clip) => {
                    let clip: AudioBuffer = clip.unchecked_into();
                    if matches!(clip.number_of_channels(), 1 | 2) {
                        s.clips.insert(id, clip);
                    }
                }
                Err(error) => warn(format!("Could not decode sound {id}: {error:?}")),
            }
        });
    }
}

#[wasm_bindgen]
impl SceneAudio {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates or resumes the audio context; call from a pointer or key gesture.
    pub fn unlock(&self) {
        let context = {
            let mut s = self.0.borrow_mut();
            if s.context.is_none() {
                let Ok(context) = AudioContext::new() else {
                    warn("Audio is unavailable".into());
                    return;
                };
                if let Ok(master) = context.create_gain() {
                    master.gain().set_value(0.8);
                    let _ = master.connect_with_audio_node(&context.destination());
                    s.master = Some(master);
                }
                s.context = Some(context);
                s.apply_layers();
            }
            s.context.clone()
        };
        let Some(context) = context else {
            return;
        };
        let state = self.0.clone();
        spawn_local(async move {
            if context.state() == AudioContextState::Suspended {
                if let Ok(promise) = context.resume() {
                    if let Err(error) = JsFuture::from(promise).await {
                        warn(format!("Audio unlock failed: {error:?}"));
                    }
                }
            }
            decode_all(&state);
        });
    }

    /// Takes the scene's preloaded sound clips (`kind: sound`, preload) from `cache` (URL to bytes) relative to
    /// `base`; they decode once audio is unlocked.
    pub fn load(&self, scene: JsValue, base: &str, cache: JsValue) {
        self.clear();
        let assets = Reflect::get(&Reflect::get(&scene, &"resources".into()).unwrap_or(JsValue::UNDEFINED), &"assets".into())
            .unwrap_or(JsValue::UNDEFINED);
        let assets = to_json(&assets);
        let get = |url: &str| -> Option<Uint8Array> {
            let method: js_sys::Function = Reflect::get(&cache, &"get".into()).ok()?.dyn_into().ok()?;
            let bytes = method.call1(&cache, &url.into()).ok()?;
            bytes.is_instance_of::<Uint8Array>().then(|| bytes.unchecked_into())
        };
        if let Some(assets) = assets.as_object() {
            for (id, asset) in assets {
                let preload = asset.get("load").and_then(serde_json::Value::as_str).unwrap_or("preload") == "preload";
                if asset.get("kind").and_then(serde_json::Value::as_str) != Some("sound") || !preload {
                    continue;
                }
                let Some(uri) = asset.get("uri").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Ok(url) = web_sys::Url::new_with_base(uri, base) else {
                    continue;
                };
                if let Some(bytes) = get(&url.href()) {
                    self.0.borrow_mut().encoded.insert(id.clone(), bytes);
                }
            }
        }
        decode_all(&self.0);
    }

    /// Applies an `audio.layer` event.
    #[wasm_bindgen(js_name = setLayer)]
    pub fn set_layer(&self, event: JsValue) {
        let Some((name, layer)) = audio_mix::layer_event(&to_json(&event)) else {
            return;
        };
        let mut s = self.0.borrow_mut();
        s.layers.entry(name).or_insert((layer, None)).0 = layer;
        s.apply_layers();
    }

    /// Plays an `audio.play` event (`{asset, layer, settings}`); false when it cannot play.
    pub fn play(&self, event: JsValue) -> bool {
        let event = to_json(&event);
        let mut s = self.0.borrow_mut();
        let Some(context) = s.context.clone().filter(|c| c.state() == AudioContextState::Running) else {
            return false;
        };
        let asset = event.get("asset").and_then(serde_json::Value::as_str).unwrap_or_default();
        let layer = event.get("layer").and_then(serde_json::Value::as_str).unwrap_or_default();
        let (Some(clip), Some(layer_node), Some(settings)) = (
            s.clips.get(asset).cloned(),
            s.layers.get(layer).and_then(|(_, node)| node.clone()),
            event.get("settings").and_then(audio_mix::voice),
        ) else {
            return false;
        };
        if s.voices.len() >= MAX_VOICES {
            if let Some(oldest) = s.voices.front().map(|v| v.id) {
                if let Some(voice) = s.voices.front() {
                            let _ = scheduled(&voice.source).stop();
                }
                s.dispose(oldest);
            }
        }
        let build = || -> Result<(AudioBufferSourceNode, Vec<AudioNode>), JsValue> {
            let source = context.create_buffer_source()?;
            let split = context.create_channel_splitter_with_number_of_outputs(2)?;
            let left = context.create_gain()?;
            let right = context.create_gain()?;
            let merge = context.create_channel_merger_with_number_of_inputs(2)?;
            left.gain().set_value(settings.left as f32);
            right.gain().set_value(settings.right as f32);
            source.set_buffer(Some(&clip));
            source.set_loop(settings.looped);
            source.playback_rate().set_value(settings.rate as f32);
            source.connect_with_audio_node(&split)?;
            split.connect_with_audio_node_and_output(&left, 0)?;
            split.connect_with_audio_node_and_output(&right, if clip.number_of_channels() == 1 { 0 } else { 1 })?;
            left.connect_with_audio_node_and_output_and_input(&merge, 0, 0)?;
            right.connect_with_audio_node_and_output_and_input(&merge, 0, 1)?;
            merge.connect_with_audio_node(&layer_node)?;
            let nodes = vec![source.clone().into(), split.into(), left.into(), right.into(), merge.into()];
            Ok((source, nodes))
        };
        let Ok((source, nodes)) = build() else {
            return false;
        };
        let id = s.next_voice;
        s.next_voice += 1;
        if s.on_ended.is_none() {
            // One handler for every voice: it finds the finished voice by its source node.
            let state = Rc::downgrade(&self.0);
            s.on_ended = Some(Closure::new(move |event: web_sys::Event| {
                let (Some(state), Some(target)) = (state.upgrade(), event.target()) else {
                    return;
                };
                let mut s = state.borrow_mut();
                let id = s.voices.iter().find(|voice| JsValue::from(&voice.source) == JsValue::from(&target)).map(|voice| voice.id);
                if let Some(id) = id {
                    s.dispose(id);
                }
            }));
        }
        scheduled(&source).set_onended(s.on_ended.as_ref().map(|handler| handler.as_ref().unchecked_ref()));
        if scheduled(&source).start().is_err() {
            return false;
        }
        s.voices.push_back(Voice { source, nodes, id });
        s.played += 1;
        true
    }

    /// Stops every playing voice (the menu opened).
    #[wasm_bindgen(js_name = stopVoices)]
    pub fn stop_voices(&self) {
        self.0.borrow_mut().stop_voices();
    }

    /// Stops every voice and forgets the scene's clips and layers.
    pub fn clear(&self) {
        let mut s = self.0.borrow_mut();
        s.generation += 1;
        s.stop_voices();
        s.encoded.clear();
        s.clips.clear();
        s.decoding.clear();
        for (_, node) in s.layers.values() {
            if let Some(node) = node {
                node.disconnect().ok();
            }
        }
        s.layers.clear();
    }

    pub fn diagnostics(&self) -> JsValue {
        let s = self.0.borrow();
        let state = s.context.as_ref().map_or("locked".to_string(), |c| format!("{:?}", c.state()).to_lowercase());
        JSON::parse(&serde_json::json!({"state": state, "clips": s.clips.len(), "voices": s.voices.len(), "played": s.played}).to_string())
            .unwrap_or(JsValue::NULL)
    }
}
