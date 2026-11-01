//! Short left-then-right output-device check. Run with headphones or stereo speakers.
use countersplat_audio::{AudioClip, AudioConfig, AudioMixer, PlaySettings};
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
    let mut audio = AudioMixer::open(AudioConfig::default())?;
    println!("Playing the left channel...");
    audio.play(
        "ui",
        &clip,
        PlaySettings {
            balance: -1.0,
            ..Default::default()
        },
    )?;
    std::thread::sleep(Duration::from_millis(550));
    println!("Playing the right channel...");
    audio.play(
        "ui",
        &clip,
        PlaySettings {
            balance: 1.0,
            ..Default::default()
        },
    )?;
    std::thread::sleep(Duration::from_millis(550));
    Ok(())
}
