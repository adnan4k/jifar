//! Domain types and orchestration.
//!
//! Tauri command handlers in `src-tauri` stay thin and call into this crate.

pub mod library;
pub mod notes;
pub mod paths;
pub mod provider_keys;
pub mod recording;
pub mod settings;
pub mod tasks;
pub mod transcription;
