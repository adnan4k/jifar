//! Audio capture, encoding and offline chunking (SPEC §6.3).
//!
//! Invariants:
//! - Never depends on `jifar-providers`, `jifar-queue` or any HTTP client.
//!   Recording must work with no network (enforced by `scripts/check-deps.sh`).
//! - Real-time callbacks only push into the lock-free ring buffer: no
//!   allocation, locks, logging, I/O or `await`.
//! - Audio is written incrementally (Ogg/Opus pages); a crash loses at most ~5 s.
//! - VAD/chunking runs offline after recording stops (SPEC D1), never in the
//!   capture path.
