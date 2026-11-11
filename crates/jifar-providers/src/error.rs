use std::time::Duration;

/// What the user has to fix before any request can succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Missing, invalid or revoked API key (401/403, or no key stored).
    Auth,
    /// Prepaid balance too low (402).
    Credits,
}

/// Failure classes the queue acts on (SPEC §6.4, §6.5).
///
/// - `Retryable`: try the same request again later (network, 429, 5xx,
///   still processing). The queue backs off, honouring `retry_after`.
/// - `ActionRequired`: no request can succeed until the user acts. The queue
///   pauses everything for this provider (as when offline) and the UI asks.
/// - `Permanent`: this chunk can't succeed as is (bad audio, unsupported
///   language). The job fails; other jobs continue.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("temporary failure: {message}")]
    Retryable {
        message: String,
        retry_after: Option<Duration>,
    },
    #[error("action required ({kind:?}): {message}")]
    ActionRequired { kind: ActionKind, message: String },
    #[error("permanent failure: {message}")]
    Permanent { message: String },
}

impl ProviderError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self::Permanent {
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable { .. })
    }
}
