//! `AppError`: the only error type crossing IPC (SPEC §6.2).
//! The UI branches on `code`, never on `message`, so codes are stable API.

use jifar_core::paths::PathsError;
use jifar_core::recording::RecordingError;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    PathsUnavailable,
    /// `recording_start` while a recording is in progress.
    AlreadyRecording,
    /// Pause/resume/stop with nothing recording.
    NotRecording,
    /// The meeting ID is not the one being recorded.
    MeetingNotFound,
    /// No audio server, no device could be opened, or capture failed.
    AudioUnavailable,
    /// Reading or writing settings or meeting files failed.
    StorageFailed,
    /// A command argument was rejected (e.g. an empty API key).
    InvalidInput,
    /// The OS keyring (Secret Service) can't be used.
    KeyringUnavailable,
    /// No provider API key, or it was rejected: fix it in Settings.
    ProviderAuth,
    /// The provider balance is too low.
    ProviderCredits,
    /// Offline or the provider is busy; try again later.
    ProviderUnavailable,
    /// The provider couldn't do this request (e.g. invalid output twice).
    ProviderFailed,
}

#[derive(Debug, thiserror::Error, Serialize, specta::Type)]
#[error("{code:?}: {message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<PathsError> for AppError {
    fn from(e: PathsError) -> Self {
        Self::new(ErrorCode::PathsUnavailable, e.to_string())
    }
}

impl From<RecordingError> for AppError {
    fn from(e: RecordingError) -> Self {
        let code = match &e {
            RecordingError::AlreadyRecording => ErrorCode::AlreadyRecording,
            RecordingError::NotRecording => ErrorCode::NotRecording,
            RecordingError::WrongMeeting(_) => ErrorCode::MeetingNotFound,
            RecordingError::Audio(_) => ErrorCode::AudioUnavailable,
            RecordingError::Io(_) => ErrorCode::StorageFailed,
        };
        Self::new(code, e.to_string())
    }
}

impl From<jifar_core::provider_keys::KeyError> for AppError {
    fn from(e: jifar_core::provider_keys::KeyError) -> Self {
        use jifar_core::provider_keys::KeyError;
        let code = match &e {
            KeyError::Empty => ErrorCode::InvalidInput,
            KeyError::Keyring(_) => ErrorCode::KeyringUnavailable,
        };
        Self::new(code, e.to_string())
    }
}

impl From<jifar_core::library::LibraryError> for AppError {
    fn from(e: jifar_core::library::LibraryError) -> Self {
        use jifar_core::library::LibraryError;
        let code = match &e {
            LibraryError::NotFound(_) => ErrorCode::MeetingNotFound,
            LibraryError::EmptyTitle => ErrorCode::InvalidInput,
            LibraryError::Store(_) | LibraryError::Io(_) => ErrorCode::StorageFailed,
        };
        Self::new(code, e.to_string())
    }
}

impl From<jifar_core::notes::NotesError> for AppError {
    fn from(e: jifar_core::notes::NotesError) -> Self {
        use jifar_core::notes::NotesError;
        use jifar_providers::{ActionKind, ProviderError};
        let code = match &e {
            NotesError::NotFound(_) => ErrorCode::MeetingNotFound,
            NotesError::NoTranscript | NotesError::BadLanguage(_) => ErrorCode::InvalidInput,
            NotesError::Provider(ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                ..
            }) => ErrorCode::ProviderAuth,
            NotesError::Provider(ProviderError::ActionRequired {
                kind: ActionKind::Credits,
                ..
            }) => ErrorCode::ProviderCredits,
            NotesError::Provider(ProviderError::Retryable { .. }) => ErrorCode::ProviderUnavailable,
            NotesError::Provider(ProviderError::Permanent { .. }) => ErrorCode::ProviderFailed,
            NotesError::Store(_) | NotesError::Library(_) => ErrorCode::StorageFailed,
        };
        Self::new(code, e.to_string())
    }
}
