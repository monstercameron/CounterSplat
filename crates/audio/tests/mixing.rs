use countersplat_audio::{
    AudioClip, AudioConfig, AudioError, LayerMix, OfflineMixer, PlaySettings, VoiceStatus,
};
use std::time::Duration;

fn make_mixer() -> OfflineMixer {
    OfflineMixer::offline(
        AudioConfig {
            transition: Duration::ZERO,
            ..AudioConfig::default()
        },
        48_000,
    )
    .unwrap()
}
fn mono(value: f32) -> AudioClip {
    AudioClip::from_pcm(48_000, 1, &vec![value; 48_000]).unwrap()
}
fn tail(mixer: &mut OfflineMixer) -> countersplat_audio::StereoFrame {
    *mixer.render(1024).unwrap().last().unwrap()
}
fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.015,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn stereo_channels_survive_centered_playback_and_mono_is_duplicated() {
    let mut mixer = make_mixer();
    let stereo: Vec<f32> = (0..48_000).flat_map(|_| [0.30, -0.10]).collect();
    let clip = AudioClip::from_pcm(48_000, 2, &stereo).unwrap();
    mixer.play("music", &clip, PlaySettings::default()).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.30);
    near(frame.right, -0.10);
    mixer.set_master(0.0, false).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.0);
    near(frame.right, 0.0);
    let mut other = make_mixer();
    other
        .play("sound", &mono(0.2), PlaySettings::default())
        .unwrap();
    let frame = tail(&mut other);
    near(frame.left, 0.2);
    near(frame.right, 0.2);
}

#[test]
fn independent_layers_mix_balance_mute_solo_and_master_gain() {
    let mut mixer = make_mixer();
    let clip = mono(0.1);
    mixer
        .play(
            "music",
            &clip,
            PlaySettings {
                balance: -1.0,
                looped: true,
                ..Default::default()
            },
        )
        .unwrap();
    mixer
        .play(
            "sound",
            &clip,
            PlaySettings {
                balance: 1.0,
                looped: true,
                ..Default::default()
            },
        )
        .unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.1 * 2.0_f32.sqrt());
    near(frame.right, 0.1 * 2.0_f32.sqrt());
    mixer
        .set_layer_mix(
            "music",
            LayerMix {
                gain: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.05 * 2.0_f32.sqrt());
    near(frame.right, 0.1 * 2.0_f32.sqrt());
    mixer
        .set_layer_mix(
            "sound",
            LayerMix {
                solo: true,
                ..Default::default()
            },
        )
        .unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.0);
    near(frame.right, 0.1 * 2.0_f32.sqrt());
    mixer
        .set_layer_mix(
            "sound",
            LayerMix {
                muted: true,
                solo: true,
                ..Default::default()
            },
        )
        .unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.0);
    near(frame.right, 0.0);
    mixer.set_layer_mix("sound", LayerMix::default()).unwrap();
    mixer.set_master(0.5, false).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.025 * 2.0_f32.sqrt());
    near(frame.right, 0.05 * 2.0_f32.sqrt());
    assert_eq!(mixer.layer_mix("music").unwrap().gain, 0.5);
}

#[test]
fn voices_pause_resume_stop_and_are_scoped_to_their_mixer() {
    let mut a = make_mixer();
    let mut b = make_mixer();
    let clip = mono(0.2);
    let voice = a
        .play(
            "sound",
            &clip,
            PlaySettings {
                looped: true,
                ..Default::default()
            },
        )
        .unwrap();
    let _ = tail(&mut a);
    assert!(matches!(
        b.pause_voice(voice),
        Err(AudioError::MissingVoice)
    ));
    a.pause_voice(voice).unwrap();
    near(tail(&mut a).left, 0.0);
    assert_eq!(a.voice_status(voice).unwrap(), VoiceStatus::Paused);
    a.resume_voice(voice).unwrap();
    near(tail(&mut a).left, 0.2);
    a.stop_voice(voice).unwrap();
    let _ = tail(&mut a);
    assert_eq!(a.voice_status(voice).unwrap(), VoiceStatus::Stopped);
    assert_eq!(a.reap_finished(), 1);
    assert_eq!(a.voice_status(voice), Err(AudioError::MissingVoice));
}

#[test]
fn rejects_invalid_audio_and_unknown_layers() {
    assert!(AudioClip::from_pcm(48_000, 3, &[0.0, 0.0, 0.0]).is_err());
    assert!(AudioClip::from_pcm(48_000, 2, &[0.0]).is_err());
    assert!(AudioClip::from_pcm(48_000, 1, &[f32::NAN]).is_err());
    assert!(AudioClip::decode(vec![1, 2, 3]).is_err());
    let mut mixer = make_mixer();
    let clip = mono(0.1);
    assert!(mixer
        .play("unknown", &clip, PlaySettings::default())
        .is_err());
    assert!(mixer
        .play(
            "sound",
            &clip,
            PlaySettings {
                balance: f32::NAN,
                ..Default::default()
            }
        )
        .is_err());
    assert!(mixer
        .set_layer_mix(
            "music",
            LayerMix {
                gain: f32::INFINITY,
                ..Default::default()
            }
        )
        .is_err());
    assert!(mixer.add_layer(" music ").is_err());
    mixer.add_layer("dialogue").unwrap();
    assert!(mixer.add_layer("dialogue").is_err());
}

fn wav() -> Vec<u8> {
    let sample_count = 4096u32;
    let data_size = sample_count * 4;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&192_000u32.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    for _ in 0..sample_count {
        out.extend_from_slice(&8192i16.to_le_bytes());
        out.extend_from_slice(&(-4096i16).to_le_bytes());
    }
    out
}
#[test]
fn decodes_real_stereo_wav_bytes() {
    let clip = AudioClip::decode(wav()).unwrap();
    assert_eq!(clip.sample_rate(), 48_000);
    assert_eq!(clip.num_frames(), 4096);
    let mut mixer = make_mixer();
    mixer.play("sound", &clip, PlaySettings::default()).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.25);
    near(frame.right, -0.125);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn streams_a_wav_from_disk_without_decoding_the_whole_asset_first() {
    let path = std::env::temp_dir().join(format!("countersplat-audio-{}.wav", std::process::id()));
    std::fs::write(&path, wav()).unwrap();
    let mut mixer = make_mixer();
    let voice = mixer
        .play_stream("music", &path, PlaySettings::default())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut heard = false;
    while std::time::Instant::now() < deadline {
        let output = mixer.render(256).unwrap();
        if output.iter().any(|f| f.left > 0.1 && f.right < -0.05) {
            heard = true;
            break;
        }
        if let Some(error) = mixer.take_stream_error(voice).unwrap() {
            panic!("stream decode: {error}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::remove_file(path).unwrap();
    assert!(heard, "stream never produced stereo samples");
}

#[test]
fn reports_layer_voice_capacity_and_updates_balance_live() {
    let mut mixer = OfflineMixer::offline(
        AudioConfig {
            voices_per_layer: 1,
            transition: Duration::ZERO,
            ..Default::default()
        },
        48_000,
    )
    .unwrap();
    let clip = mono(0.1);
    let voice = mixer
        .play(
            "sound",
            &clip,
            PlaySettings {
                looped: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(
        mixer.play("sound", &clip, PlaySettings::default()),
        Err(AudioError::Capacity("voice"))
    ));
    // A full sound layer does not prevent another layer from playing.
    mixer
        .play(
            "music",
            &clip,
            PlaySettings {
                gain: 0.0,
                looped: true,
                ..Default::default()
            },
        )
        .unwrap();
    mixer.set_voice_balance(voice, -1.0).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.1 * 2.0_f32.sqrt());
    near(frame.right, 0.0);
    mixer.set_voice_balance(voice, 1.0).unwrap();
    let frame = tail(&mut mixer);
    near(frame.left, 0.0);
    near(frame.right, 0.1 * 2.0_f32.sqrt());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn preserves_background_decoder_error_after_voice_cleanup() {
    let path = std::env::temp_dir().join(format!(
        "countersplat-invalid-stream-{}.wav",
        std::process::id()
    ));
    let sample_count = 2048u32;
    let data_size = sample_count * 6;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&288_000u32.to_le_bytes());
    bytes.extend_from_slice(&6u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    bytes.resize(bytes.len() + data_size as usize, 0);
    std::fs::write(&path, bytes).unwrap();
    let mut mixer = make_mixer();
    let voice = mixer
        .play_stream("music", &path, PlaySettings::default())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && mixer.voice_status(voice).unwrap() != VoiceStatus::Stopped
    {
        let _ = mixer.render(256).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(mixer.voice_status(voice).unwrap(), VoiceStatus::Stopped);
    assert_eq!(mixer.reap_finished(), 1);
    assert!(mixer.take_stream_error(voice).unwrap().is_some());
    assert_eq!(mixer.dropped_stream_error_count(), 0);
    std::fs::remove_file(path).unwrap();
}
