use crate::{AudioConfig, AudioError, Mixer, StereoFrame};
use kira::backend::{Backend, Renderer};

/// A deterministic, device-free Kira backend. Rendering happens only when requested by the host.
pub struct OfflineBackend {
    renderer: Option<Renderer>,
}
impl Backend for OfflineBackend {
    type Settings = u32;
    type Error = &'static str;
    fn setup(sample_rate: u32, _buffer_size: usize) -> Result<(Self, u32), Self::Error> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err("offline sample rate must be 8,000..=384,000 Hz");
        }
        Ok((Self { renderer: None }, sample_rate))
    }
    fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
        self.renderer = Some(renderer);
        Ok(())
    }
}
/// The same mixer and commands as device playback, with explicitly requested stereo output.
pub type OfflineMixer = Mixer<OfflineBackend>;
impl Mixer<OfflineBackend> {
    pub fn offline(config: AudioConfig, sample_rate: u32) -> Result<Self, AudioError> {
        Self::create(config, sample_rate)
    }
    /// Render up to one million stereo frames. Intended for tests and offline export, not an audio callback.
    pub fn render(&mut self, frames: usize) -> Result<Vec<StereoFrame>, AudioError> {
        if frames > 1_000_000 {
            return Err(AudioError::Invalid(
                "offline render exceeds one million frames",
            ));
        }
        let mut samples = vec![0.0; frames * 2];
        let renderer = self
            .manager
            .backend_mut()
            .renderer
            .as_mut()
            .ok_or(AudioError::Device("offline renderer is unavailable".into()))?;
        renderer.on_start_processing();
        renderer.process(&mut samples, 2);
        Ok(samples
            .chunks_exact(2)
            .map(|pair| StereoFrame::new(pair[0], pair[1]))
            .collect())
    }
}
