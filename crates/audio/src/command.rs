use crate::{AudioClip, AudioError, LayerMix, Mixer, PlaySettings, VoiceId, VoiceStatus};
use kira::backend::Backend;

/// Owned commands can cross a bounded subsystem queue without sharing the audio owner.
#[derive(Debug, Clone)]
pub enum AudioCommand {
    AddLayer {
        name: String,
    },
    RemoveLayer {
        name: String,
    },
    SetLayer {
        name: String,
        mix: LayerMix,
    },
    SetMaster {
        gain: f32,
        muted: bool,
    },
    Play {
        layer: String,
        clip: AudioClip,
        settings: PlaySettings,
    },
    #[cfg(not(target_arch = "wasm32"))]
    PlayStream {
        layer: String,
        path: std::path::PathBuf,
        settings: PlaySettings,
    },
    GetLayer {
        name: String,
    },
    GetMaster,
    GetVoiceStatus {
        voice: VoiceId,
    },
    #[cfg(not(target_arch = "wasm32"))]
    TakeStreamError {
        voice: VoiceId,
    },
    SetVoiceGain {
        voice: VoiceId,
        gain: f32,
    },
    SetVoiceBalance {
        voice: VoiceId,
        balance: f32,
    },
    SetVoiceRate {
        voice: VoiceId,
        rate: f64,
    },
    PauseVoice {
        voice: VoiceId,
    },
    ResumeVoice {
        voice: VoiceId,
    },
    StopVoice {
        voice: VoiceId,
    },
    PauseLayer {
        name: String,
    },
    ResumeLayer {
        name: String,
    },
    ReapFinished,
}
#[derive(Debug, Clone, PartialEq)]
pub enum AudioReply {
    Applied,
    Voice(VoiceId),
    Layer(LayerMix),
    Master {
        gain: f32,
        muted: bool,
    },
    VoiceStatus(VoiceStatus),
    #[cfg(not(target_arch = "wasm32"))]
    StreamError(Option<String>),
    Reaped(usize),
}
impl<B: Backend> Mixer<B>
where
    B::Error: std::fmt::Debug,
{
    /// Apply one command on the mixer's owning thread.
    pub fn apply(&mut self, command: AudioCommand) -> Result<AudioReply, AudioError> {
        match command {
            AudioCommand::AddLayer { name } => self.add_layer(&name)?,
            AudioCommand::RemoveLayer { name } => self.remove_layer(&name)?,
            AudioCommand::SetLayer { name, mix } => self.set_layer_mix(&name, mix)?,
            AudioCommand::SetMaster { gain, muted } => self.set_master(gain, muted)?,
            AudioCommand::Play {
                layer,
                clip,
                settings,
            } => return Ok(AudioReply::Voice(self.play(&layer, &clip, settings)?)),
            #[cfg(not(target_arch = "wasm32"))]
            AudioCommand::PlayStream {
                layer,
                path,
                settings,
            } => return Ok(AudioReply::Voice(self.play_stream(&layer, path, settings)?)),
            AudioCommand::GetLayer { name } => {
                return Ok(AudioReply::Layer(self.layer_mix(&name)?))
            }
            AudioCommand::GetMaster => {
                let (gain, muted) = self.master();
                return Ok(AudioReply::Master { gain, muted });
            }
            AudioCommand::GetVoiceStatus { voice } => {
                return Ok(AudioReply::VoiceStatus(self.voice_status(voice)?))
            }
            #[cfg(not(target_arch = "wasm32"))]
            AudioCommand::TakeStreamError { voice } => {
                return Ok(AudioReply::StreamError(self.take_stream_error(voice)?))
            }
            AudioCommand::SetVoiceGain { voice, gain } => self.set_voice_gain(voice, gain)?,
            AudioCommand::SetVoiceBalance { voice, balance } => {
                self.set_voice_balance(voice, balance)?
            }
            AudioCommand::SetVoiceRate { voice, rate } => self.set_voice_rate(voice, rate)?,
            AudioCommand::PauseVoice { voice } => self.pause_voice(voice)?,
            AudioCommand::ResumeVoice { voice } => self.resume_voice(voice)?,
            AudioCommand::StopVoice { voice } => self.stop_voice(voice)?,
            AudioCommand::PauseLayer { name } => self.pause_layer(&name)?,
            AudioCommand::ResumeLayer { name } => self.resume_layer(&name)?,
            AudioCommand::ReapFinished => return Ok(AudioReply::Reaped(self.reap_finished())),
        }
        Ok(AudioReply::Applied)
    }
}
