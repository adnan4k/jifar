//! [`TrackRecorder`]: one capture source → ring buffer → worker thread →
//! incremental Ogg/Opus file. One instance per track (`mic`, `system`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::opus_writer::{DurableFile, OggOpusWriter};
use crate::ring::{DEFAULT_RING_CAPACITY, Overruns, RingReader, ring};
use crate::source::CaptureSource;
use crate::{AudioError, Result, SAMPLE_RATE};

/// How long the worker sleeps when the ring is empty.
const IDLE_WAIT: Duration = Duration::from_millis(5);
/// Worker read buffer (~85 ms of audio).
const READ_BUF: usize = 4096;

/// Summary of a finished track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackStats {
    pub samples: u64,
    pub duration_ms: u64,
    /// Samples dropped because the ring was full. Must be 0 (NFR-02).
    pub overrun_samples: u64,
}

pub struct TrackRecorder {
    source: Box<dyn CaptureSource>,
    path: PathBuf,
    worker: Option<JoinHandle<Result<u64>>>,
    stop: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    overruns: Overruns,
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
        let (sink, reader, overruns) = ring(ring_capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicU64::new(0));

        let worker = {
            let stop = Arc::clone(&stop);
            let samples = Arc::clone(&samples);
            thread::Builder::new()
                .name(format!("audio-worker:{}", source.name()))
                .spawn(move || run_worker(reader, writer, &stop, &samples))?
        };
        source.start(sink)?;
        info!(source = source.name(), path = %path.display(), "track recording started");

        Ok(Self {
            source,
            path: path.to_owned(),
            worker: Some(worker),
            stop,
            samples,
            overruns,
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
        self.overruns.samples()
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
            overrun_samples: self.overruns.samples(),
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

fn run_worker(
    mut reader: RingReader,
    mut writer: OggOpusWriter<DurableFile>,
    stop: &AtomicBool,
    samples: &AtomicU64,
) -> Result<u64> {
    let mut buf = vec![0.0_f32; READ_BUF];
    loop {
        let n = reader.pop_into(&mut buf);
        if n > 0 {
            writer.write_samples(&buf[..n])?;
            samples.store(writer.samples_written(), Ordering::Relaxed);
            continue;
        }
        // The source is stopped before `stop` is set, so an empty ring after
        // seeing the flag means everything has been drained.
        if stop.load(Ordering::Acquire) && reader.is_empty() {
            break;
        }
        thread::sleep(IDLE_WAIT);
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
