//! [`TrackRecorder`]: one capture source → ring buffer → worker thread →
//! drift correction → incremental Ogg/Opus file. One instance per track
//! (`mic`, `system`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::drift::DriftCorrector;
use crate::opus_writer::{DurableFile, OggOpusWriter};
use crate::ring::{CaptureStats, DEFAULT_RING_CAPACITY, RingReader, ring};
use crate::source::CaptureSource;
use crate::{AudioError, Result, SAMPLE_RATE};

/// How long the worker sleeps when the ring is empty.
const IDLE_WAIT: Duration = Duration::from_millis(5);
/// Worker read buffer (~85 ms of audio).
const READ_BUF: usize = 4096;
/// How often the worker re-measures drift.
const DRIFT_INTERVAL: Duration = Duration::from_millis(250);

/// Summary of a finished track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackStats {
    /// Samples in the file, including drift correction and inserted silence.
    pub samples: u64,
    pub duration_ms: u64,
    /// Samples dropped because the ring was full. Must be 0 (NFR-02).
    pub overrun_samples: u64,
    /// Net samples added (positive) or removed by drift correction, in ms.
    pub drift_correction_ms: i64,
    /// Silence inserted to cover device stalls, in ms (part of the above).
    pub silence_inserted_ms: u64,
}

/// Drift-correction counters the worker publishes, in µs.
#[derive(Debug, Default)]
struct DriftCounters {
    correction_us: AtomicI64,
    estimate_us: AtomicI64,
    silence_us: AtomicU64,
}

pub struct TrackRecorder {
    source: Box<dyn CaptureSource>,
    path: PathBuf,
    worker: Option<JoinHandle<Result<u64>>>,
    stop: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    drift: Arc<DriftCounters>,
    stats: CaptureStats,
}

impl TrackRecorder {
    /// Creates `path`, starts the encoder worker, then starts `source`.
    pub fn start(source: Box<dyn CaptureSource>, path: &Path) -> Result<Self> {
        Self::start_with_capacity(source, path, DEFAULT_RING_CAPACITY)
    }

    pub fn start_with_capacity(
        mut source: Box<dyn CaptureSource>,
        path: &Path,
        ring_capacity: usize,
    ) -> Result<Self> {
        let writer = OggOpusWriter::new(DurableFile::create(path)?, serial_for(path))?;
        let corrector = if source.is_realtime() {
            Some(DriftCorrector::new()?)
        } else {
            None
        };
        let (sink, reader, stats) = ring(ring_capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicU64::new(0));
        let drift = Arc::new(DriftCounters::default());

        let worker = {
            let ctx = WorkerCtx {
                stop: Arc::clone(&stop),
                samples: Arc::clone(&samples),
                drift: Arc::clone(&drift),
                stats: stats.clone(),
            };
            thread::Builder::new()
                .name(format!("audio-worker:{}", source.name()))
                .spawn(move || run_worker(reader, writer, corrector, &ctx))?
        };
        if let Err(e) = source.start(sink) {
            // Without this the worker would wait for samples forever.
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
        info!(source = source.name(), path = %path.display(), "track recording started");

        Ok(Self {
            source,
            path: path.to_owned(),
            worker: Some(worker),
            stop,
            samples,
            drift,
            stats,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Samples encoded so far.
    pub fn samples_written(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    pub fn overrun_samples(&self) -> u64 {
        self.stats.overrun_samples()
    }

    /// Net timeline correction applied so far (ms, positive: added).
    pub fn drift_correction_ms(&self) -> f64 {
        self.drift.correction_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// Drift measured by the corrector (lower envelope, relative to the
    /// start of measurement), in ms. Jitter-free, unlike
    /// [`crate::TrackStatus::drift_ms`].
    pub fn drift_estimate_ms(&self) -> f64 {
        self.drift.estimate_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// Live capture counters (first-sample time, delivered samples, overruns).
    pub fn capture_stats(&self) -> &CaptureStats {
        &self.stats
    }

    /// Error reported by the capture source (e.g. the device went away).
    /// Audio already captured is still written; see [`CaptureSource::error`].
    pub fn source_error(&self) -> Option<String> {
        self.source.error()
    }

    /// See [`CaptureSource::is_finished`].
    pub fn is_source_finished(&self) -> bool {
        self.source.is_finished()
    }

    /// False if the worker exited early (I/O error or panic). Audio already on
    /// disk is intact up to the last flushed page; [`Self::stop`] reports why.
    pub fn is_healthy(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }

    /// Stops the source, drains the ring, finalizes and syncs the file.
    pub fn stop(mut self) -> Result<TrackStats> {
        let source_result = self.source.stop();
        self.stop.store(true, Ordering::Release);
        let samples = match self.worker.take().map(JoinHandle::join) {
            Some(Ok(result)) => result?,
            Some(Err(_)) => return Err(AudioError::WorkerPanicked),
            None => self.samples_written(),
        };
        source_result?;
        let stats = TrackStats {
            samples,
            duration_ms: samples * 1000 / u64::from(SAMPLE_RATE),
            overrun_samples: self.stats.overrun_samples(),
            drift_correction_ms: self.drift.correction_us.load(Ordering::Relaxed) / 1000,
            silence_inserted_ms: self.drift.silence_us.load(Ordering::Relaxed) / 1000,
        };
        if stats.overrun_samples > 0 {
            warn!(path = %self.path.display(), overruns = stats.overrun_samples, "track had ring-buffer overruns");
        }
        info!(path = %self.path.display(), ?stats, "track recording stopped");
        Ok(stats)
    }
}

impl Drop for TrackRecorder {
    fn drop(&mut self) {
        // Dropped without `stop()`: still stop capture and let the worker
        // finalize the file rather than leaving it unterminated.
        if self.worker.is_some() {
            let _ = self.source.stop();
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

struct WorkerCtx {
    stop: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    drift: Arc<DriftCounters>,
    stats: CaptureStats,
}

fn run_worker(
    mut reader: RingReader,
    mut writer: OggOpusWriter<DurableFile>,
    mut corrector: Option<DriftCorrector>,
    ctx: &WorkerCtx,
) -> Result<u64> {
    let mut buf = vec![0.0_f32; READ_BUF];
    let mut next_drift_check = Instant::now() + DRIFT_INTERVAL;
    let mut write = |block: &[f32]| -> Result<()> {
        writer.write_samples(block)?;
        ctx.samples
            .store(writer.samples_written(), Ordering::Relaxed);
        Ok(())
    };
    loop {
        let now = Instant::now();
        if now >= next_drift_check {
            next_drift_check = now + DRIFT_INTERVAL;
            if let (Some(c), Some(first)) = (corrector.as_mut(), ctx.stats.first_sample_at()) {
                let wall_ms = now.saturating_duration_since(first).as_secs_f64() * 1000.0;
                c.observe(wall_ms, ctx.stats.delivered_samples(), &mut write)?;
                publish_drift(c, &ctx.drift);
            }
        }
        let n = reader.pop_into(&mut buf);
        if n > 0 {
            match corrector.as_mut() {
                Some(c) => c.push(&buf[..n], &mut write)?,
                None => write(&buf[..n])?,
            }
            continue;
        }
        // The source is stopped before `stop` is set, so an empty ring after
        // seeing the flag means everything has been drained.
        if ctx.stop.load(Ordering::Acquire) && reader.is_empty() {
            break;
        }
        thread::sleep(IDLE_WAIT);
    }
    if let Some(c) = corrector.as_mut() {
        c.flush(&mut write)?;
        publish_drift(c, &ctx.drift);
    }
    let total = writer.samples_written();
    writer.finish()?.close()?;
    debug!(total, "audio worker finished");
    Ok(total)
}

/// Ogg stream serial derived from the path, so `mic.opus` and `system.opus`
/// get different serials (they could be muxed later without clashes).
fn serial_for(path: &Path) -> u32 {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
        })
}

fn publish_drift(corrector: &DriftCorrector, counters: &DriftCounters) {
    let us = |ms: f64| (ms * 1000.0).round() as i64;
    counters
        .correction_us
        .store(us(corrector.correction_ms()), Ordering::Relaxed);
    counters
        .estimate_us
        .store(us(corrector.estimate_ms()), Ordering::Relaxed);
    counters.silence_us.store(
        corrector.silence_samples() * 1_000_000 / u64::from(SAMPLE_RATE),
        Ordering::Relaxed,
    );
}
