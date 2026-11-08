use thiserror::Error;

/// Errors produced by the capture, encoding and decoding pipeline.
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("opus codec error: {0}")]
    Opus(#[from] opus::Error),
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
    #[error("invalid ogg/opus stream: {0}")]
    InvalidStream(String),
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),
    #[error("capture source error: {0}")]
    Source(String),
    #[error("capture source is already running")]
    AlreadyRunning,
    #[error("audio worker thread panicked")]
    WorkerPanicked,
}

pub type Result<T, E = AudioError> = std::result::Result<T, E>;
