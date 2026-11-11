//! [`TrackRecorder`]: one capture source → ring buffer → worker thread →
//! drift correction → level meter → incremental Ogg/Opus file. One instance
//! per track (`mic`, `system`).

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::drift::DriftCorrector;
use crate::level::{Level, LevelMeter, LevelShared};
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
    levels: Arc<LevelShared>,
    fault: Arc<OnceLock<String>>,
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
        let levels = Arc::new(LevelShared::default());
        let fault = Arc::new(OnceLock::new());

        let worker = {
            let ctx = WorkerCtx {
                stop: Arc::clone(&stop),
                samples: Arc::clone(&samples),
                drift: Arc::clone(&drift),
                levels: Arc::clone(&levels),
                fault: Arc::clone(&fault),
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
            levels,
            fault,
            stats,
        })
    }

    /// Why the encoder worker stopped early (panic or write error), if it
    /// did. Capture continues but nothing more is written; the file holds
    /// everything up to the fault and is finalized if possible.
    pub fn fault(&self) -> Option<String> {
        self.fault.get().cloned()
    }

    /// Level of the last 100 ms written (FR-REC-08). `None` until the first
    /// 100 ms of audio has been written.
    pub fn level(&self) -> Option<Level> {
        self.levels.level()
    }

    /// Current run of silence (below [`crate::SILENCE_DBFS`]) in ms of audio.
    pub fn silent_ms(&self) -> u64 {
        self.levels.silent_ms()
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
            // `run_worker` catches panics itself; this is a panic that
            // escaped even that (e.g. while finalizing).
            Some(Err(payload)) => {
                return Err(AudioError::WorkerPanicked(panic_message(&*payload)));
            }
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
    levels: Arc<LevelShared>,
    fault: Arc<OnceLock<String>>,
    stats: CaptureStats,
}

/// Runs the encoder loop. A panic or write error in the loop is caught, the
/// fault is published, and the file is still finalized when possible, so a
/// bug costs at most the audio after it, never the recording so far.
fn run_worker(
    mut reader: RingReader,
    mut writer: OggOpusWriter<DurableFile>,
    mut corrector: Option<DriftCorrector>,
    ctx: &WorkerCtx,
) -> Result<u64> {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        pump(&mut reader, &mut writer, &mut corrector, ctx)
    }));
    let fault = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(payload) => Some(AudioError::WorkerPanicked(panic_message(&*payload))),
    };
    if let Some(e) = &fault {
        error!(error = %e, "audio worker failed; finalizing what was recorded");
        let _ = ctx.fault.set(e.to_string());
    }

    let total = writer.samples_written();
    // The writer may be mid-page after a panic; finalizing must not panic
    // the thread again.
    let finalized = panic::catch_unwind(AssertUnwindSafe(|| {
        writer.finish()?.close().map_err(AudioError::from)
    }));
    match (fault, finalized) {
        (None, Ok(Ok(()))) => {
            debug!(total, "audio worker finished");
            Ok(total)
        }
        (None, Ok(Err(e))) => Err(e),
        (None, Err(payload)) => Err(AudioError::WorkerPanicked(format!(
            "while finalizing: {}",
            panic_message(&*payload)
        ))),
        (Some(e), Ok(Ok(()))) => {
            warn!(total, "file finalized after worker fault");
            Err(match e {
                AudioError::WorkerPanicked(m) => {
                    AudioError::WorkerPanicked(format!("{m} (file finalized)"))
                }
                other => other,
            })
        }
        (Some(e), _) => {
            warn!("file could not be finalized; it decodes up to the last flushed page");
            Err(match e {
                AudioError::WorkerPanicked(m) => AudioError::WorkerPanicked(format!(
                    "{m} (file not finalized; readable up to the last page)"
                )),
                other => other,
            })
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Test-only: a sample with exactly these bits makes the worker panic.
#[cfg(test)]
pub(crate) const PANIC_SENTINEL_BITS: u32 = 0x7fc0_dead;

fn pump(
    reader: &mut RingReader,
    writer: &mut OggOpusWriter<DurableFile>,
    corrector: &mut Option<DriftCorrector>,
    ctx: &WorkerCtx,
) -> Result<()> {
    let mut buf = vec![0.0_f32; READ_BUF];
    let mut next_drift_check = Instant::now() + DRIFT_INTERVAL;
    let mut meter = LevelMeter::new(Arc::clone(&ctx.levels));
    let mut write = |block: &[f32]| -> Result<()> {
        #[cfg(test)]
        if block.iter().any(|s| s.to_bits() == PANIC_SENTINEL_BITS) {
            panic!("injected test panic");
        }
        meter.feed(block);
        writer.write_samples(block)?;
        ctx.samples
            .store(writer.samples_written(), Ordering::Relaxed);
        Ok(())
    };
    loop {
        let now = Instant::now();
        if now >= next_drift_check {
            next_drift_check = now + DRIFT_INTERVAL;
            // Paused time isn't recording time: measure on the active clock,
            // and not at all while paused.
            let active = (!ctx.stats.is_paused())
                .then(|| ctx.stats.active_since_first(now))
                .flatten();
            if let (Some(c), Some(active)) = (corrector.as_mut(), active) {
                let wall_ms = active.as_secs_f64() * 1000.0;
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
    Ok(())
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
