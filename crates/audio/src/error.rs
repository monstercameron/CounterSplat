use std::fmt;

/// A recoverable configuration, asset, playback, or device failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    Invalid(&'static str),
    DuplicateLayer(String),
    MissingLayer(String),
    MissingVoice,
    Capacity(&'static str),
    Decode(String),
    Device(String),
}
impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid audio input: {reason}"),
            Self::DuplicateLayer(name) => write!(f, "audio layer already exists: {name}"),
            Self::MissingLayer(name) => write!(f, "audio layer does not exist: {name}"),
            Self::MissingVoice => f.write_str("audio voice does not exist in this mixer"),
            Self::Capacity(resource) => write!(f, "audio {resource} capacity reached"),
            Self::Decode(reason) => write!(f, "audio decode failed: {reason}"),
            Self::Device(reason) => write!(f, "audio device failed: {reason}"),
        }
    }
}
impl std::error::Error for AudioError {}
