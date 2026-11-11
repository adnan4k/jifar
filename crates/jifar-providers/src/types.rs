use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::ProviderError;

/// ISO 639-1 code (`om` = Afaan Oromo). Read it from `meeting.language`;
/// never hardcode it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LangCode(String);

impl LangCode {
    /// Lower-cases and trims; rejects anything that isn't 2–3 ASCII
    /// letters (Scribe also uses 3-letter codes such as `sid`, `wal`).
    pub fn new(code: &str) -> Option<Self> {
        let c = code.trim().to_ascii_lowercase();
        ((2..=3).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase())).then_some(Self(c))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LangCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One VAD chunk to transcribe (a row of `asr_jobs`, SPEC §7.2).
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// The job ID. Sent as the provider's idempotency key, so a retry of the
    /// same job is never billed twice.
    pub request_id: String,
    /// 16 kHz mono 16-bit WAV (`chunks/<track>-<index>.wav`).
    pub path: PathBuf,
    /// Position on the recording clock.
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A transcribed span, on the recording clock (not relative to the chunk).
#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    /// Exactly as returned: Qubee apostrophes and doubled letters intact.
    pub text: String,
    pub confidence: Option<f32>,
}

#[async_trait]
pub trait AsrProvider: Send + Sync {
    /// Stable ID stored in `asr_jobs.provider` (e.g. `"addis-scribe"`).
    fn id(&self) -> &'static str;

    /// Transcribes one chunk. An empty `Vec` means no speech was recognized.
    async fn transcribe(
        &self,
        chunk: &AudioChunk,
        language: &LangCode,
    ) -> Result<Vec<RawSegment>, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_codes_are_normalized_and_checked() {
        assert_eq!(LangCode::new(" OM ").unwrap().as_str(), "om");
        assert_eq!(LangCode::new("sid").unwrap().as_str(), "sid");
        assert!(LangCode::new("").is_none());
        assert!(LangCode::new("oromo").is_none());
        assert!(LangCode::new("o1").is_none());
    }
}
