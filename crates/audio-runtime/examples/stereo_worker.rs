//! Play a short left/right check through the dedicated audio owner.
use countersplat_audio::{AudioClip, AudioCommand, AudioConfig, PlaySettings};
use countersplat_audio_runtime::spawn_device;
use std::{f32::consts::TAU, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sample_rate = 48_000;
    let samples: Vec<_> = (0..sample_rate / 3)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let envelope = (t * 40.0).min(1.0) * (((1.0 / 3.0 - t) * 40.0).clamp(0.0, 1.0));
            0.08 * envelope * (TAU * 440.0 * t).sin()
        })
        .collect();
    let clip = AudioClip::from_pcm(sample_rate, 1, &samples)?;
    let mut worker = spawn_device(AudioConfig::default(), 16)?;
    worker.wait_ready(Duration::from_secs(3))?;
    let audio = worker.client();
    for (label, balance) in [("left", -1.0), ("right", 1.0)] {
        println!("Playing the {label} channel on the audio worker...");
        audio
            .try_submit(AudioCommand::Play {
                layer: "ui".into(),
                clip: clip.clone(),
                settings: PlaySettings {
                    balance,
                    ..Default::default()
                },
            })?
            .wait()?;
        std::thread::sleep(Duration::from_millis(550));
    }
    worker.shutdown()?;
    Ok(())
}
