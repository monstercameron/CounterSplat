use crate::{
    config::{validate_balance, validate_gain, validate_layer_name, validate_rate},
    AudioClip, AudioConfig, AudioError, LayerMix, PlaySettings, DEFAULT_LAYERS,
};
#[cfg(feature = "device")]
use kira::backend::DefaultBackend;
use kira::{
    backend::Backend,
    sound::{static_sound::StaticSoundHandle, PlaybackState},
    track::{TrackBuilder, TrackHandle},
    AudioManager, AudioManagerSettings, Decibels, PlaySoundError, Tween,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(target_arch = "wasm32"))]
use kira::sound::{
    streaming::{StreamingSoundData, StreamingSoundHandle},
    FromFileError,
};

#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PENDING_STREAM_ERRORS: usize = 256;

static NEXT_MIXER_ID: AtomicU64 = AtomicU64::new(1);

/// A process-local handle scoped to the mixer which created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId {
    mixer: u64,
    sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Playing,
    Pausing,
    Paused,
    WaitingToResume,
    Resuming,
    Stopping,
    Stopped,
}
impl From<PlaybackState> for VoiceStatus {
    fn from(value: PlaybackState) -> Self {
        match value {
            PlaybackState::Playing => Self::Playing,
            PlaybackState::Pausing => Self::Pausing,
            PlaybackState::Paused => Self::Paused,
            PlaybackState::WaitingToResume => Self::WaitingToResume,
            PlaybackState::Resuming => Self::Resuming,
            PlaybackState::Stopping => Self::Stopping,
            PlaybackState::Stopped => Self::Stopped,
        }
    }
}
struct Layer {
    mix: LayerMix,
    track: TrackHandle,
}
struct Voice {
    layer: String,
    handle: VoiceHandle,
}
enum VoiceHandle {
    Static(StaticSoundHandle),
    #[cfg(not(target_arch = "wasm32"))]
    Stream(StreamingSoundHandle<FromFileError>),
}
impl VoiceHandle {
    fn status(&self) -> VoiceStatus {
        match self {
            Self::Static(h) => h.state().into(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.state().into(),
        }
    }
    fn gain(&mut self, value: Decibels, tween: Tween) {
        match self {
            Self::Static(h) => h.set_volume(value, tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.set_volume(value, tween),
        }
    }
    fn balance(&mut self, value: f32, tween: Tween) {
        match self {
            Self::Static(h) => h.set_panning(value, tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.set_panning(value, tween),
        }
    }
    fn rate(&mut self, value: f64, tween: Tween) {
        match self {
            Self::Static(h) => h.set_playback_rate(value, tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.set_playback_rate(value, tween),
        }
    }
    fn pause(&mut self, tween: Tween) {
        match self {
            Self::Static(h) => h.pause(tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.pause(tween),
        }
    }
    fn resume(&mut self, tween: Tween) {
        match self {
            Self::Static(h) => h.resume(tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.resume(tween),
        }
    }
    fn stop(&mut self, tween: Tween) {
        match self {
            Self::Static(h) => h.stop(tween),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Stream(h) => h.stop(tween),
        }
    }
}
fn playback_error<E: std::fmt::Debug>(error: PlaySoundError<E>) -> AudioError {
    match error {
        PlaySoundError::SoundLimitReached => AudioError::Capacity("voice"),
        PlaySoundError::IntoSoundError(error) => AudioError::Decode(format!("{error:?}")),
    }
}
fn decibels(gain: f32) -> Decibels {
    if gain <= 0.0 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * gain.log10())
    }
}

/// Audio mixer state. Keep this on a subsystem owner thread; do not call it from an audio callback.
pub struct Mixer<B: Backend> {
    pub(crate) manager: AudioManager<B>,
    config: AudioConfig,
    mixer_id: u64,
    next_voice: u64,
    layers: HashMap<String, Layer>,
    voices: HashMap<VoiceId, Voice>,
    master_gain: f32,
    master_muted: bool,
    #[cfg(not(target_arch = "wasm32"))]
    stream_errors: VecDeque<(VoiceId, String)>,
    #[cfg(not(target_arch = "wasm32"))]
    dropped_stream_errors: usize,
}

/// The real output-device mixer. On the web, construct it during a user gesture.
#[cfg(feature = "device")]
pub type AudioMixer = Mixer<DefaultBackend>;

#[cfg(feature = "device")]
impl Mixer<DefaultBackend> {
    /// Open the platform's default output device, returning an error when unavailable.
    pub fn open(config: AudioConfig) -> Result<Self, AudioError> {
        Self::create(config, Default::default())
    }
}

impl<B: Backend> Mixer<B>
where
    B::Error: std::fmt::Debug,
{
    pub(crate) fn create(
        config: AudioConfig,
        backend_settings: B::Settings,
    ) -> Result<Self, AudioError> {
        config.validate()?;
        let mixer_id = NEXT_MIXER_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| AudioError::Capacity("mixer identifier"))?;
        let mut settings = AudioManagerSettings::<B> {
            backend_settings,
            capacities: Default::default(),
            main_track_builder: Default::default(),
            internal_buffer_size: 128,
        };
        settings.capacities.sub_track_capacity = config.max_layers;
        let manager =
            AudioManager::new(settings).map_err(|e| AudioError::Device(format!("{e:?}")))?;
        let mut mixer = Self {
            manager,
            config,
            mixer_id,
            next_voice: 1,
            layers: HashMap::new(),
            voices: HashMap::new(),
            master_gain: 1.0,
            master_muted: false,
            #[cfg(not(target_arch = "wasm32"))]
            stream_errors: VecDeque::new(),
            #[cfg(not(target_arch = "wasm32"))]
            dropped_stream_errors: 0,
        };
        for name in DEFAULT_LAYERS {
            mixer.add_layer(name)?;
        }
        Ok(mixer)
    }
    fn tween(&self) -> Tween {
        Tween {
            duration: self.config.transition,
            ..Default::default()
        }
    }
    fn next_id(&mut self) -> Result<VoiceId, AudioError> {
        let sequence = self.next_voice;
        self.next_voice = sequence
            .checked_add(1)
            .ok_or(AudioError::Capacity("voice identifier"))?;
        Ok(VoiceId {
            mixer: self.mixer_id,
            sequence,
        })
    }
    /// Add a uniquely named layer. Layer names are independent of ECS and asset IDs.
    pub fn add_layer(&mut self, name: &str) -> Result<(), AudioError> {
        validate_layer_name(name)?;
        if self.layers.contains_key(name) {
            return Err(AudioError::DuplicateLayer(name.to_owned()));
        }
        if self.layers.len() >= self.config.max_layers {
            return Err(AudioError::Capacity("layer"));
        }
        let solo_active = self.layers.values().any(|l| l.mix.solo);
        let track = self
            .manager
            .add_sub_track(
                TrackBuilder::new()
                    .sound_capacity(self.config.voices_per_layer)
                    .volume(if solo_active {
                        Decibels::SILENCE
                    } else {
                        Decibels::IDENTITY
                    }),
            )
            .map_err(|_| AudioError::Capacity("layer"))?;
        self.layers.insert(
            name.to_owned(),
            Layer {
                mix: LayerMix::default(),
                track,
            },
        );
        Ok(())
    }
    /// Remove a layer and stop its voices. Other layers are unaffected.
    pub fn remove_layer(&mut self, name: &str) -> Result<(), AudioError> {
        self.layers
            .remove(name)
            .ok_or_else(|| AudioError::MissingLayer(name.to_owned()))?;
        self.voices.retain(|_, voice| voice.layer != name);
        self.apply_layer_mix();
        Ok(())
    }
    pub fn layer_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.layers.keys().cloned().collect();
        names.sort();
        names
    }
    pub fn layer_mix(&self, name: &str) -> Result<LayerMix, AudioError> {
        Ok(self
            .layers
            .get(name)
            .ok_or_else(|| AudioError::MissingLayer(name.to_owned()))?
            .mix)
    }
    /// Atomically replace one layer's gain, mute, and solo settings on the control owner.
    pub fn set_layer_mix(&mut self, name: &str, mix: LayerMix) -> Result<(), AudioError> {
        mix.validate()?;
        self.layers
            .get_mut(name)
            .ok_or_else(|| AudioError::MissingLayer(name.to_owned()))?
            .mix = mix;
        self.apply_layer_mix();
        Ok(())
    }
    fn apply_layer_mix(&mut self) {
        let any_solo = self.layers.values().any(|layer| layer.mix.solo);
        let tween = self.tween();
        for layer in self.layers.values_mut() {
            let audible = !layer.mix.muted && (!any_solo || layer.mix.solo);
            layer
                .track
                .set_volume(decibels(if audible { layer.mix.gain } else { 0.0 }), tween);
        }
    }
    /// Set master gain and mute without altering the individual layer settings.
    pub fn set_master(&mut self, gain: f32, muted: bool) -> Result<(), AudioError> {
        validate_gain(gain)?;
        self.master_gain = gain;
        self.master_muted = muted;
        let tween = self.tween();
        self.manager
            .main_track()
            .set_volume(decibels(if muted { 0.0 } else { gain }), tween);
        Ok(())
    }
    pub fn master(&self) -> (f32, bool) {
        (self.master_gain, self.master_muted)
    }
    /// Start a reusable in-memory clip on a named layer.
    pub fn play(
        &mut self,
        layer: &str,
        clip: &AudioClip,
        options: PlaySettings,
    ) -> Result<VoiceId, AudioError> {
        options.validate()?;
        if !self.layers.contains_key(layer) {
            return Err(AudioError::MissingLayer(layer.to_owned()));
        }
        self.reap_finished();
        let id = self.next_id()?;
        let mut data = clip
            .data
            .volume(decibels(options.gain))
            .panning(options.balance)
            .playback_rate(options.rate);
        if options.looped {
            data = data.loop_region(..);
        }
        let handle = self
            .layers
            .get_mut(layer)
            .expect("layer was checked")
            .track
            .play(data)
            .map_err(playback_error)?;
        self.voices.insert(
            id,
            Voice {
                layer: layer.to_owned(),
                handle: VoiceHandle::Static(handle),
            },
        );
        Ok(id)
    }
    /// Open and stream a long file on native platforms. The decoder performs I/O outside the audio callback.
    /// File opening and header decoding happen on this control thread; use an audio/asset owner worker.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn play_stream(
        &mut self,
        layer: &str,
        path: impl AsRef<std::path::Path>,
        options: PlaySettings,
    ) -> Result<VoiceId, AudioError> {
        options.validate()?;
        if !self.layers.contains_key(layer) {
            return Err(AudioError::MissingLayer(layer.to_owned()));
        }
        let mut data = StreamingSoundData::from_file(path)
            .map_err(|e| AudioError::Decode(e.to_string()))?
            .volume(decibels(options.gain))
            .panning(options.balance)
            .playback_rate(options.rate);
        if options.looped {
            data = data.loop_region(..);
        }
        self.reap_finished();
        let id = self.next_id()?;
        let handle = self
            .layers
            .get_mut(layer)
            .expect("layer was checked")
            .track
            .play(data)
            .map_err(playback_error)?;
        self.voices.insert(
            id,
            Voice {
                layer: layer.to_owned(),
                handle: VoiceHandle::Stream(handle),
            },
        );
        Ok(id)
    }
    fn voice_mut(&mut self, id: VoiceId) -> Result<&mut VoiceHandle, AudioError> {
        if id.mixer != self.mixer_id {
            return Err(AudioError::MissingVoice);
        }
        Ok(&mut self
            .voices
            .get_mut(&id)
            .ok_or(AudioError::MissingVoice)?
            .handle)
    }
    pub fn voice_status(&self, id: VoiceId) -> Result<VoiceStatus, AudioError> {
        if id.mixer != self.mixer_id {
            return Err(AudioError::MissingVoice);
        }
        Ok(self
            .voices
            .get(&id)
            .ok_or(AudioError::MissingVoice)?
            .handle
            .status())
    }
    pub fn set_voice_gain(&mut self, id: VoiceId, gain: f32) -> Result<(), AudioError> {
        validate_gain(gain)?;
        let t = self.tween();
        self.voice_mut(id)?.gain(decibels(gain), t);
        Ok(())
    }
    pub fn set_voice_balance(&mut self, id: VoiceId, balance: f32) -> Result<(), AudioError> {
        validate_balance(balance)?;
        let t = self.tween();
        self.voice_mut(id)?.balance(balance, t);
        Ok(())
    }
    pub fn set_voice_rate(&mut self, id: VoiceId, rate: f64) -> Result<(), AudioError> {
        validate_rate(rate)?;
        let t = self.tween();
        self.voice_mut(id)?.rate(rate, t);
        Ok(())
    }
    pub fn pause_voice(&mut self, id: VoiceId) -> Result<(), AudioError> {
        let t = self.tween();
        self.voice_mut(id)?.pause(t);
        Ok(())
    }
    pub fn resume_voice(&mut self, id: VoiceId) -> Result<(), AudioError> {
        let t = self.tween();
        self.voice_mut(id)?.resume(t);
        Ok(())
    }
    pub fn stop_voice(&mut self, id: VoiceId) -> Result<(), AudioError> {
        let t = self.tween();
        self.voice_mut(id)?.stop(t);
        Ok(())
    }
    pub fn pause_layer(&mut self, name: &str) -> Result<(), AudioError> {
        let t = self.tween();
        self.layers
            .get_mut(name)
            .ok_or_else(|| AudioError::MissingLayer(name.to_owned()))?
            .track
            .pause(t);
        Ok(())
    }
    pub fn resume_layer(&mut self, name: &str) -> Result<(), AudioError> {
        let t = self.tween();
        self.layers
            .get_mut(name)
            .ok_or_else(|| AudioError::MissingLayer(name.to_owned()))?
            .track
            .resume(t);
        Ok(())
    }
    /// Return a pending background decoder error for a streaming voice, if any.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_stream_error(&mut self, id: VoiceId) -> Result<Option<String>, AudioError> {
        if id.mixer != self.mixer_id {
            return Err(AudioError::MissingVoice);
        }
        if let Some(index) = self
            .stream_errors
            .iter()
            .position(|(voice, _)| *voice == id)
        {
            return Ok(self.stream_errors.remove(index).map(|(_, error)| error));
        }
        match self.voice_mut(id)? {
            VoiceHandle::Static(_) => Ok(None),
            VoiceHandle::Stream(h) => Ok(h.pop_error().map(|e| e.to_string())),
        }
    }
    /// Number of old decoder errors discarded when the bounded error queue filled.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn dropped_stream_error_count(&self) -> usize {
        self.dropped_stream_errors
    }
    /// Remove completed voice handles, preserving any pending stream errors for later polling.
    pub fn reap_finished(&mut self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut errors = Vec::new();
            for (id, voice) in &mut self.voices {
                if voice.handle.status() == VoiceStatus::Stopped {
                    if let VoiceHandle::Stream(handle) = &mut voice.handle {
                        while let Some(error) = handle.pop_error() {
                            errors.push((*id, error.to_string()));
                        }
                    }
                }
            }
            for error in errors {
                if self.stream_errors.len() == MAX_PENDING_STREAM_ERRORS {
                    self.stream_errors.pop_front();
                    self.dropped_stream_errors = self.dropped_stream_errors.saturating_add(1);
                }
                self.stream_errors.push_back(error);
            }
        }
        let before = self.voices.len();
        self.voices
            .retain(|_, voice| voice.handle.status() != VoiceStatus::Stopped);
        before - self.voices.len()
    }
    pub fn tracked_voices(&self) -> usize {
        self.voices.len()
    }
}
