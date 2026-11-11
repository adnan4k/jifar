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
//! [`CaptureSource`] → [`RtSink`] (lock-free ring) → worker thread (drift
//! correction) → [`OggOpusWriter`] → [`DurableFile`], orchestrated by [`TrackRecorder`].
//! A [`RecordingSession`] runs the `mic` and `system` tracks on one clock.
//!
//! After stop, [`chunk_track`] turns each archive into 16 kHz ASR chunks
//! (decode → [`Vad`] → WAV), see the `chunker` module.
//!
//! Sources: `PipeWireSource` (feature `pipewire`) and `PulseSource`
//! (feature `pulse`), both on by default, for real devices, chosen at runtime
//! by [`Backend::detect`]; [`FakeSource`] for tests and fixtures.

mod backend;
mod chunker;
mod device;
mod drift;
mod error;
mod fake;
mod level;
mod mix;
mod opus_reader;
mod opus_writer;
#[cfg(feature = "pulse")]
mod pulse_source;
#[cfg(feature = "pipewire")]
mod pw_source;
mod recorder;
mod resample;
mod ring;
mod session;
mod source;
mod vad;

pub use backend::{Backend, CaptureTarget};
pub use chunker::{Chunk, ChunkReport, ChunkerConfig, chunk_file_name, chunk_track};
pub use device::{AudioDevice, DeviceChoice, DeviceKind, resolve_device};
pub use error::{AudioError, Result};
pub use fake::{FakeSource, Pacing};
pub use level::{LEVEL_WINDOW, Level, MIN_DBFS, SILENCE_DBFS, SILENCE_WARNING_MS, to_dbfs};
pub use mix::{MixInput, MixReport, render_mixed};
pub use opus_reader::OggOpusReader;
pub use opus_writer::{BITRATE_BPS, DurableFile, OggOpusWriter, SYNC_INTERVAL};
#[cfg(feature = "pulse")]
pub use pulse_source::PulseSource;
#[cfg(feature = "pipewire")]
pub use pw_source::PipeWireSource;
pub use recorder::{TrackRecorder, TrackStats};
pub use resample::CHUNK_SAMPLE_RATE;
pub use ring::{CaptureStats, DEFAULT_RING_CAPACITY, RingReader, RtSink, ring};
pub use session::{
    Pause, RecordingSession, SessionStatus, SessionSummary, Track, TrackStatus, TrackSummary,
};
pub use source::CaptureSource;
pub use vad::{EnergyVad, SileroVad, VAD_WINDOW, VAD_WINDOW_MS, Vad};

/// Capture and archive sample rate (Hz). Everything in the capture path is
/// 48 kHz mono f32.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per Opus frame (20 ms at 48 kHz).
pub const FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 50;
