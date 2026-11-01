use crate::AudioError;
use kira::{
    sound::static_sound::{StaticSoundData, StaticSoundSettings},
    Frame,
};
use std::{io::Cursor, sync::Arc, time::Duration};

/// Maximum encoded asset accepted by [`AudioClip::decode`].
pub const MAX_ENCODED_BYTES: usize = 128 * 1024 * 1024;
/// Maximum decoded frames held by one static clip (ten minutes at 48 kHz).
pub const MAX_CLIP_FRAMES: usize = 48_000 * 60 * 10;

/// A pair of left/right samples at one point in time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}
impl StereoFrame {
    pub const fn new(left: f32, right: f32) -> Self {
        Self { left, right }
    }
}

/// Reusable decoded audio; clones share their sample storage.
#[derive(Clone)]
pub struct AudioClip {
    pub(crate) data: Arc<StaticSoundData>,
}
impl std::fmt::Debug for AudioClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioClip")
            .field("sample_rate", &self.sample_rate())
            .field("frames", &self.num_frames())
            .finish()
    }
}
impl AudioClip {
    /// Decode a complete WAV, MP3, Ogg/Vorbis, or FLAC asset away from the audio callback.
    /// The encoded-size limit is checked before decoding; the decoded-frame limit is checked
    /// afterwards, so only decode trusted assets. Long native music can use streaming instead.
    pub fn decode(bytes: Vec<u8>) -> Result<Self, AudioError> {
        if bytes.is_empty() || bytes.len() > MAX_ENCODED_BYTES {
            return Err(AudioError::Invalid(
                "encoded clip is empty or exceeds 128 MiB",
            ));
        }
        let data = StaticSoundData::from_cursor(Cursor::new(bytes))
            .map_err(|e| AudioError::Decode(e.to_string()))?;
        Self::check(data)
    }
    /// Load and decode a complete file. Do file I/O on an asset/worker thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, AudioError> {
        let file = std::fs::File::open(path).map_err(|e| AudioError::Decode(e.to_string()))?;
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take((MAX_ENCODED_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| AudioError::Decode(e.to_string()))?;
        Self::decode(bytes)
    }
    /// Build a clip from interleaved mono or stereo PCM. Mono is duplicated to both channels.
    pub fn from_pcm(sample_rate: u32, channels: u16, samples: &[f32]) -> Result<Self, AudioError> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(AudioError::Invalid(
                "sample rate must be 8,000..=384,000 Hz",
            ));
        }
        if !matches!(channels, 1 | 2) {
            return Err(AudioError::Invalid(
                "only mono and stereo PCM are supported",
            ));
        }
        if samples.is_empty()
            || !samples.len().is_multiple_of(channels as usize)
            || samples.len() / channels as usize > MAX_CLIP_FRAMES
        {
            return Err(AudioError::Invalid("invalid PCM frame count"));
        }
        if samples
            .iter()
            .any(|sample| !sample.is_finite() || sample.abs() > 8.0)
        {
            return Err(AudioError::Invalid(
                "PCM samples must be finite and within -8..=8",
            ));
        }
        let frames: Arc<[Frame]> = samples
            .chunks_exact(channels as usize)
            .map(|frame| Frame::new(frame[0], if channels == 1 { frame[0] } else { frame[1] }))
            .collect::<Vec<_>>()
            .into();
        Self::check(StaticSoundData {
            sample_rate,
            frames,
            settings: StaticSoundSettings::default(),
            slice: None,
        })
    }
    fn check(data: StaticSoundData) -> Result<Self, AudioError> {
        if !(8_000..=384_000).contains(&data.sample_rate)
            || data.frames.is_empty()
            || data.frames.len() > MAX_CLIP_FRAMES
        {
            return Err(AudioError::Invalid(
                "decoded clip has an unsupported sample rate or frame count",
            ));
        }
        if data.frames.iter().any(|f| {
            !f.left.is_finite() || !f.right.is_finite() || f.left.abs() > 8.0 || f.right.abs() > 8.0
        }) {
            return Err(AudioError::Invalid("decoded clip contains invalid samples"));
        }
        Ok(Self {
            data: Arc::new(data),
        })
    }
    pub fn sample_rate(&self) -> u32 {
        self.data.sample_rate
    }
    pub fn num_frames(&self) -> usize {
        self.data.frames.len()
    }
    pub fn duration(&self) -> Duration {
        self.data.duration()
    }
}
