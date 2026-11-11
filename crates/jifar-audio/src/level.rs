//! Live level metering and silence detection (FR-REC-08).
//!
//! Runs on the encoder worker over the stream that is written to the file
//! (after drift correction), never in the real-time callback. Every 100 ms of
//! audio it publishes RMS and peak in dBFS, which gives the ~10 Hz rate of
//! the `audio://levels` event (SPEC §6.2). Because stall silence is metered
//! too, a device that stops delivering also ends up "silent" and triggers the
//! warning. Paused time is not metered: nothing is written while paused.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::SAMPLE_RATE;

/// Samples per metering window (100 ms).
pub const LEVEL_WINDOW: usize = SAMPLE_RATE as usize / 10;
/// Windows whose RMS is below this count as silence. Above a muted or dead
/// input (digital zero), below the noise floor of a normal room.
pub const SILENCE_DBFS: f32 = -60.0;
/// Continuous silence after which a track is flagged (SPEC FR-REC-08).
pub const SILENCE_WARNING_MS: u64 = 30_000;
/// Lowest reported level; digital silence is clamped here (no `-inf`).
pub const MIN_DBFS: f32 = -100.0;

const WINDOW_MS: u64 = (LEVEL_WINDOW as u64 * 1000) / SAMPLE_RATE as u64;

/// Level of the last 100 ms window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
}

/// Converts a linear amplitude (1.0 = full scale) to dBFS, floored at
/// [`MIN_DBFS`].
pub fn to_dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return MIN_DBFS;
    }
    (20.0 * amplitude.log10()).max(MIN_DBFS)
}

/// Values the worker publishes for readers on other threads.
#[derive(Debug)]
pub(crate) struct LevelShared {
    rms_bits: AtomicU32,
    peak_bits: AtomicU32,
    silent_ms: AtomicU64,
}

impl Default for LevelShared {
    fn default() -> Self {
        Self {
            rms_bits: AtomicU32::new(f32::NAN.to_bits()),
            peak_bits: AtomicU32::new(f32::NAN.to_bits()),
            silent_ms: AtomicU64::new(0),
        }
    }
}

impl LevelShared {
    /// `None` until the first window has been measured.
    pub(crate) fn level(&self) -> Option<Level> {
        let rms = f32::from_bits(self.rms_bits.load(Ordering::Relaxed));
        let peak = f32::from_bits(self.peak_bits.load(Ordering::Relaxed));
        (!rms.is_nan()).then_some(Level {
            rms_dbfs: rms,
            peak_dbfs: peak,
        })
    }

    /// Length of the current run of silence, in ms of audio.
    pub(crate) fn silent_ms(&self) -> u64 {
        self.silent_ms.load(Ordering::Relaxed)
    }
}

pub(crate) struct LevelMeter {
    shared: Arc<LevelShared>,
    sum_sq: f64,
    peak: f32,
    n: usize,
    silent_ms: u64,
}

impl LevelMeter {
    pub(crate) fn new(shared: Arc<LevelShared>) -> Self {
        Self {
            shared,
            sum_sq: 0.0,
            peak: 0.0,
            n: 0,
            silent_ms: 0,
        }
    }

    pub(crate) fn feed(&mut self, mut samples: &[f32]) {
        while !samples.is_empty() {
            let take = (LEVEL_WINDOW - self.n).min(samples.len());
            for &s in &samples[..take] {
                self.sum_sq += f64::from(s) * f64::from(s);
                self.peak = self.peak.max(s.abs());
            }
            self.n += take;
            samples = &samples[take..];
            if self.n == LEVEL_WINDOW {
                self.publish();
            }
        }
    }

    fn publish(&mut self) {
        let rms = to_dbfs((self.sum_sq / LEVEL_WINDOW as f64).sqrt() as f32);
        let peak = to_dbfs(self.peak);
        self.silent_ms = if rms < SILENCE_DBFS {
            self.silent_ms + WINDOW_MS
        } else {
            0
        };
        let s = &self.shared;
        s.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        s.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        s.silent_ms.store(self.silent_ms, Ordering::Relaxed);
        self.sum_sq = 0.0;
        self.peak = 0.0;
        self.n = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn meter() -> (LevelMeter, Arc<LevelShared>) {
        let shared = Arc::new(LevelShared::default());
        (LevelMeter::new(Arc::clone(&shared)), shared)
    }

    #[test]
    fn nothing_until_first_window() {
        let (mut m, shared) = meter();
        m.feed(&[0.5; LEVEL_WINDOW - 1]);
        assert_eq!(shared.level(), None);
        m.feed(&[0.5]);
        assert!(shared.level().is_some());
    }

    #[test]
    fn full_scale_sine_is_minus_3_rms_0_peak() {
        let (mut m, shared) = meter();
        let sine: Vec<f32> = (0..LEVEL_WINDOW)
            .map(|i| (TAU * 1000.0 * i as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        m.feed(&sine);
        let level = shared.level().unwrap();
        assert!((level.rms_dbfs + 3.01).abs() < 0.05, "{level:?}");
        assert!(level.peak_dbfs.abs() < 0.01, "{level:?}");
    }

    #[test]
    fn digital_silence_is_floored_not_infinite() {
        let (mut m, shared) = meter();
        m.feed(&[0.0; LEVEL_WINDOW]);
        let level = shared.level().unwrap();
        assert_eq!(level.rms_dbfs, MIN_DBFS);
        assert_eq!(level.peak_dbfs, MIN_DBFS);
    }

    #[test]
    fn silence_run_counts_up_and_resets_on_sound() {
        let (mut m, shared) = meter();
        // -70 dBFS noise floor: silent.
        let quiet = [10f32.powf(-70.0 / 20.0); LEVEL_WINDOW];
        for _ in 0..301 {
            m.feed(&quiet);
        }
        assert_eq!(shared.silent_ms(), 30_100);
        assert!(shared.silent_ms() > SILENCE_WARNING_MS);
        // -40 dBFS speech-ish: not silent.
        m.feed(&[0.01; LEVEL_WINDOW]);
        assert_eq!(shared.silent_ms(), 0);
    }
}
