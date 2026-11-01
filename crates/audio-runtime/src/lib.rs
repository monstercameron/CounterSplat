//! Optional adapter that owns an audio mixer on a bounded, named native worker.
//! The device callback remains owned by Kira/CPAL; no locks or filesystem work are added to it.
//! WebAssembly has no native workers here: browser hosts must use an appropriate Web Audio host.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    #[cfg(feature = "device")]
    use countersplat_audio::AudioMixer;
    use countersplat_audio::{AudioCommand, AudioConfig, AudioReply, OfflineMixer};
    use countersplat_runtime::native::{SpawnError, Worker};

    /// Bounded request queue, readiness, response tickets, and explicit shutdown.
    pub type AudioWorker = Worker<AudioCommand, AudioReply>;

    /// Start the default output device on a dedicated audio control thread.
    /// Call `wait_ready` to observe device initialization; do not block a window event loop on tickets.
    #[cfg(feature = "device")]
    pub fn spawn_device(
        config: AudioConfig,
        queue_capacity: usize,
    ) -> Result<AudioWorker, SpawnError> {
        Worker::spawn(
            "audio",
            queue_capacity,
            move || AudioMixer::open(config).map_err(|error| error.to_string()),
            |mixer: &mut AudioMixer, command| {
                mixer.apply(command).map_err(|error| error.to_string())
            },
        )
    }

    /// Start the same control API with a device-free backend, useful for headless hosts.
    /// This worker does not advance or produce samples on its own.
    pub fn spawn_offline(
        config: AudioConfig,
        sample_rate: u32,
        queue_capacity: usize,
    ) -> Result<AudioWorker, SpawnError> {
        Worker::spawn(
            "audio-offline",
            queue_capacity,
            move || OfflineMixer::offline(config, sample_rate).map_err(|error| error.to_string()),
            |mixer: &mut OfflineMixer, command| {
                mixer.apply(command).map_err(|error| error.to_string())
            },
        )
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
