//! [`FakeSource`]: a [`CaptureSource`] that plays samples or a WAV fixture,
//! so the pipeline can be tested in CI without PipeWire.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ring::RtSink;
use crate::source::CaptureSource;
use crate::{AudioError, Result, SAMPLE_RATE};

/// Samples delivered per simulated callback (10 ms at 48 kHz).
const BLOCK: usize = SAMPLE_RATE as usize / 100;

/// How a [`FakeSource`] paces delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// One 10 ms block every 10 ms, like a real device. A full ring drops
    /// samples and counts overruns, exactly as a device callback would.
    Realtime,
    /// As fast as the worker drains, waiting while the ring is full. Never
    /// overruns. Useful for fast tests and offline encoding.
    Lossless,
}

pub struct FakeSource {
    name: String,
    samples: Arc<[f32]>,
    pacing: Pacing,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeSource {
    pub fn from_samples(name: impl Into<String>, samples: Vec<f32>, pacing: Pacing) -> Self {
        Self {
            name: name.into(),
            samples: samples.into(),
            pacing,
            stop: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Loads a 48 kHz WAV fixture (int or float, any channel count; channels
    /// are averaged down to mono). Other sample rates are rejected: resampling
    /// is not the fixture's job.
    pub fn from_wav(path: &Path, pacing: Pacing) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.sample_rate != SAMPLE_RATE {
            return Err(AudioError::UnsupportedFormat(format!(
                "{}: {} Hz, expected {SAMPLE_RATE} Hz",
                path.display(),
                spec.sample_rate
            )));
        }
        let channels = usize::from(spec.channels.max(1));
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1_i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 * scale))
                    .collect::<Result<_, _>>()?
            }
        };
        let mono = interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect();
        Ok(Self::from_samples(path.display().to_string(), mono, pacing))
    }

    pub fn len_samples(&self) -> usize {
        self.samples.len()
    }
}

impl CaptureSource for FakeSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, mut sink: RtSink) -> Result<()> {
        if self.thread.is_some() {
            return Err(AudioError::AlreadyRunning);
        }
        let samples = Arc::clone(&self.samples);
        let stop = Arc::clone(&self.stop);
        let finished = Arc::clone(&self.finished);
        let pacing = self.pacing;
        let handle = thread::Builder::new()
            .name(format!("fake-capture:{}", self.name))
            .spawn(move || {
                let started = Instant::now();
                for (i, block) in samples.chunks(BLOCK).enumerate() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match pacing {
                        Pacing::Realtime => {
                            let due = started + Duration::from_millis(10 * i as u64);
                            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                                thread::sleep(wait);
                            }
                            sink.push(block);
                        }
                        Pacing::Lossless => {
                            let mut rest = block;
                            while !rest.is_empty() && !stop.load(Ordering::Relaxed) {
                                if sink.is_abandoned() {
                                    return;
                                }
                                let n = sink.push(&rest[..rest.len().min(sink.free_slots())]);
                                rest = &rest[n..];
                                if !rest.is_empty() {
                                    thread::sleep(Duration::from_millis(1));
                                }
                            }
                        }
                    }
                }
                finished.store(true, Ordering::Release);
            })?;
        self.thread = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            handle
                .join()
                .map_err(|_| AudioError::Source(format!("fake source {} panicked", self.name)))?;
        }
        Ok(())
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
