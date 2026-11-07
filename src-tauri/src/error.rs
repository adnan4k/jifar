//! `AppError`: the only error type crossing IPC (SPEC §6.2).
//! The UI branches on `code`, never on `message`, so codes are stable API.

use jifar_core::paths::PathsError;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    PathsUnavailable,
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
