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
//!
//! Pipeline per track:
//! [`CaptureSource`] → [`RtSink`] (lock-free ring) → worker thread →
//! [`OggOpusWriter`] → [`DurableFile`], orchestrated by [`TrackRecorder`].

mod error;
mod fake;
mod opus_reader;
mod opus_writer;
mod recorder;
mod ring;
mod source;

pub use error::{AudioError, Result};
pub use fake::{FakeSource, Pacing};
pub use opus_reader::OggOpusReader;
pub use opus_writer::{BITRATE_BPS, DurableFile, OggOpusWriter, SYNC_INTERVAL};
pub use recorder::{TrackRecorder, TrackStats};
pub use ring::{DEFAULT_RING_CAPACITY, Overruns, RingReader, RtSink, ring};
pub use source::CaptureSource;

/// Capture and archive sample rate (Hz). Everything in the capture path is
/// 48 kHz mono f32.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per Opus frame (20 ms at 48 kHz).
pub const FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 50;
