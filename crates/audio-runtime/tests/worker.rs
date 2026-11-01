#![cfg(not(target_arch = "wasm32"))]
use countersplat_audio::{
    AudioClip, AudioCommand, AudioConfig, AudioReply, LayerMix, PlaySettings,
};
use countersplat_audio_runtime::spawn_offline;
use std::time::Duration;

#[test]
fn worker_accepts_owned_commands_and_shuts_down() {
    let mut worker = spawn_offline(AudioConfig::default(), 48_000, 8).unwrap();
    worker.wait_ready(Duration::from_secs(2)).unwrap();
    let client = worker.client();
    let mix = LayerMix {
        gain: 0.4,
        muted: false,
        solo: false,
    };
    assert_eq!(
        client
            .try_submit(AudioCommand::SetLayer {
                name: "music".into(),
                mix
            })
            .unwrap()
            .wait()
            .unwrap(),
        AudioReply::Applied
    );
    assert_eq!(
        client
            .try_submit(AudioCommand::GetLayer {
                name: "music".into()
            })
            .unwrap()
            .wait()
            .unwrap(),
        AudioReply::Layer(mix)
    );
    let clip = AudioClip::from_pcm(48_000, 1, &vec![0.1; 1024]).unwrap();
    let reply = client
        .try_submit(AudioCommand::Play {
            layer: "music".into(),
            clip,
            settings: PlaySettings::default(),
        })
        .unwrap()
        .wait()
        .unwrap();
    assert!(matches!(reply, AudioReply::Voice(_)));
    worker.shutdown().unwrap();
}
