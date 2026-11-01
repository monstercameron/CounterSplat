# Audio

`countersplat-audio` is an independent, Kira-backed audio library. It owns a stereo mixer and uses CPAL for output and Symphonia for decoding. `countersplat-audio-ecs` is a separate adapter for scene/ECS and Lua access; neither the mixer nor ECS depends on the other. The built-in layers are `music`, `sound`, `ambience`, `voice`, and `ui`; applications may add named layers up to the configured limit.

## Playback and mixing

Enable the `device` feature in a host (`countersplat-audio = { workspace = true, features = ["device"] }`). The crate itself enables it by default; the workspace dependency leaves it off so headless and ECS-only hosts do not open or require a device.

```rust,no_run
use countersplat_audio::{AudioClip, AudioConfig, AudioMixer, LayerMix, PlaySettings};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut audio = AudioMixer::open(AudioConfig::default())?;
let clip = AudioClip::from_file("assets/sound/step.wav")?;
let voice = audio.play("sound", &clip, PlaySettings {
    gain: 0.6,
    balance: -0.4,
    ..Default::default()
})?;
audio.set_voice_balance(voice, 0.4)?;
audio.set_layer_mix("music", LayerMix { gain: 0.5, ..Default::default() })?;
audio.set_master(0.8, false)?;
# Ok(())
# }
```

Gain is linear, from `0` through `2`; `1` is unchanged and `0` is silence. Master, layer and voice gains multiply. Each layer has independent gain, mute and solo; when any layer is soloed, non-solo layers are silent, and muting still takes precedence. Voice handles support pause, resume, stop, gain, balance and playback rate. The configurable transition defaults to 10 ms to reduce clicks. The host can call `reap_finished` to discard finished handles; starting another sound also reaps them. Handles belong to one mixer and cannot be used with another.

Balance ranges from `-1` (left) through `0` (center) to `1` (right), using Kira's equal-power channel balance. Mono clips are copied into both channels. Centered stereo clips retain their original distinct left/right samples; balance attenuates/amplifies those channels, it does not cross-feed one channel into the other. Surround input is rejected rather than guessed. Kira averages left and right for a mono output device; on devices with more than two channels it sends stereo to the first two and silence to remaining channels. Mixed signals above full scale are clipped at output: preserve headroom when combining layers; there is no master limiter yet.

Short, reusable WAV/MP3/Ogg-Vorbis/FLAC clips can be decoded from bytes, files, or mono/stereo PCM. Clones share decoded samples. Decoding and file I/O belong on an asset or control worker, never in an audio callback. Encoded inputs are limited to 128 MiB and completed static clips to 28.8 million frames; the latter limit is applied after decoding, so only load trusted files. On native platforms `play_stream` streams long files on a decoder worker instead of loading the entire track. Use `take_stream_error` to receive background decoder errors. Reaping preserves pending errors in a bounded queue; `dropped_stream_error_count` reports overflow. Browser builds support static clips from fetched bytes; Kira does not support streaming clips on wasm.

## Ownership and scene hooks

The audio mixer lives on its owner thread. Kira/CPAL calls its renderer on the device's audio thread. `AudioCommand` and `AudioReply` carry owned requests and results. Enable the `device` feature of `countersplat-audio-runtime` and call `spawn_device` to create the native audio owner with a bounded queue and nonblocking tickets. Its `spawn_offline` variant is useful in headless hosts. See the working [audio scene integration test](../apps/threaded-demo/tests/audio_scene.rs). Browser hosts must create audio from a user gesture to satisfy browser autoplay rules, and keep it on the thread that created it.

Scene manifests may declare `AudioLayer` and `AudioEmitter` as native components, with audio resources referenced by asset ID:

```json
{
  "component_types": {
    "AudioLayer": { "storage": "native" },
    "AudioEmitter": { "storage": "native" }
  },
  "resources": {
    "assets": { "step": { "kind": "sound", "uri": "../sound/step.wav" } }
  },
  "entities": [
    { "id": "effects", "layer": "audio", "components": { "AudioLayer": { "name": "sound" } } },
    { "id": "step-source", "layer": "audio", "components": {
      "AudioEmitter": { "asset": "step", "layer": "sound", "autoplay": true }
    } }
  ]
}
```

Register the audio ECS adapters before instantiating the `audio` layer, then extract an owned snapshot on the ECS owner. Lua uses the existing `ecs.get` and `ecs.set` bridge with validation. The host must resolve scene asset IDs, compare snapshots, send mixer commands, and implement `autoplay` lifecycle behavior. The current browser gameplay host does not yet perform this reconciliation or start audio. The adapter supplies the hooks; it does not claim the scene is already playing sound.

## Validation and examples

`OfflineMixer` runs the same Kira mixer without an output device and returns stereo frames for deterministic tests or offline use. `cargo test -p countersplat-audio` covers independent layers, stereo, live balance, voice lifetime, limits, WAV decoding and native streaming. `cargo run -p countersplat-audio-runtime --example stereo_worker` plays a short quiet tone left then right through the default native output device on the audio owner thread. The direct-mixer equivalent is `cargo run -p countersplat-audio --example stereo_check`. Device loss/reconnection, 3D positional emitters, listener transforms, and automatic gameplay-host integration remain future work.
