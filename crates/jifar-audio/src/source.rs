//! Capture sources: anything that delivers 48 kHz mono f32 into an [`RtSink`].

use crate::Result;
use crate::ring::RtSink;

/// A capture device (PipeWire/PulseAudio stream) or a test double.
///
/// Implementations deliver 48 kHz mono f32 samples. Their real-time callback
/// may only call [`RtSink::push`]: no allocation, locks, logging, I/O or
/// `await` on that path.
pub trait CaptureSource: Send {
    /// Human-readable name for logs (device description, fixture path).
    fn name(&self) -> &str;

    /// Starts delivering samples into `sink`. Fails with
    /// [`crate::AudioError::AlreadyRunning`] if already started.
    fn start(&mut self, sink: RtSink) -> Result<()>;

    /// Stops delivery. After it returns, no more samples are pushed.
    /// Idempotent.
    fn stop(&mut self) -> Result<()>;

    /// True when a finite source (e.g. a fixture file) has delivered all of
    /// its audio. Live devices never finish on their own.
    fn is_finished(&self) -> bool {
        false
    }

    /// True if samples arrive at the device's real rate, so wall time and
    /// delivered audio can be compared. Drift correction only runs for
    /// real-time sources; faster-than-real-time feeds (offline encoding,
    /// fixtures) are written untouched.
    fn is_realtime(&self) -> bool {
        true
    }

    /// A fault the source hit while running (stream error, device removed,
    /// unexpected format). Capture may have stopped; samples already pushed
    /// are still encoded. `None` while healthy.
    fn error(&self) -> Option<String> {
        None
    }
}
