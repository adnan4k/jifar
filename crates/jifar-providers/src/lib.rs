//! `AsrProvider` / `NotesProvider` traits and implementations (SPEC §6.5).
//!
//! This is the only crate allowed to make network calls. Errors must be
//! classified as `Retryable` or `Permanent`; the queue depends on it.
