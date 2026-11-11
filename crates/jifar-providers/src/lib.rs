//! `AsrProvider` / `NotesProvider` traits and implementations (SPEC §6.5).
//!
//! This is the only crate allowed to make network calls. Every failure is
//! classified (see [`ProviderError`]) because the transcription queue decides
//! what to do from the class alone: retry, pause and ask the user, or give up
//! on that chunk.
//!
//! Provider output is untrusted: segments are validated and clamped to the
//! chunk before they leave this crate.

pub mod addis;
pub mod addis_notes;
mod error;
pub mod keys;
pub mod notes;
mod types;

pub use addis::{AddisScribe, Usage};
pub use addis_notes::AddisNotes;
pub use error::{ActionKind, ProviderError};
pub use keys::{
    ADDIS_AI_ACCOUNT, ApiKey, ApiKeyStore, KeyStoreError, KeyringStore, MemoryKeyStore,
};
pub use notes::{ActionItem, Decision, MeetingNotes, NotesInput, NotesProvider, NotesSegment};
pub use types::{AsrProvider, AudioChunk, LangCode, RawSegment};
