//! Persistent ASR job queue: `pending → in_flight → done/failed`, backoff with
//! jitter, restart recovery and connectivity handling (SPEC §6.4).
//!
//! Jobs are created only after recording stops (SPEC D1).
