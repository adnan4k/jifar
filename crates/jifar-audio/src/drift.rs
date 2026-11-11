//! Drift correction (FR-REC-04, SPEC §6.3), on the encoder worker thread.
//!
//! A capture device's clock is never exactly 48 kHz, and the mic and the
//! system sink are often different devices, so their tracks slowly move apart
//! (typically 10–200 ppm, i.e. up to ~0.7 s per hour). Every track is held
//! to the session's monotonic clock, which keeps the tracks aligned with each
//! other and with every timestamp.
//!
//! - **Measure.** The *lag* is wall time since the track's first sample minus
//!   the audio time the device has delivered. Delivery jitter only ever makes
//!   audio arrive late, so the minimum lag over a short window (lower
//!   envelope) follows the true clock trend and ignores the jitter. Drift is
//!   that envelope relative to its value when measurement starts.
//! - **Correct small drift.** Once the uncorrected drift exceeds 20 ms, the
//!   stream is rate-matched with rubato's [`Slip`]: it inserts or drops single
//!   frames behind a short crossfade (inaudible at these rates, no delay, an
//!   exact pass-through while idle). It stops once the error is below 2 ms.
//! - **Fill stalls.** A device that stops delivering for a while (Bluetooth
//!   hiccup, suspended node) shows as a persistent step in the lag. Steps above
//!   250 ms are filled with silence, since rate matching would take minutes.
//!   The silence lands up to one envelope window (2 s) after the real gap,
//!   possibly in a few steps as the envelope catches up with it.
//!
//! Never runs in the real-time callback: it is driven by the worker, which
//! already does all non-real-time work.

use std::collections::VecDeque;

use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{Adjustable, FixedAsync, Indexing, Resampler, Slip};

use crate::{AudioError, Result, SAMPLE_RATE};

/// Drift (ms) above which correction starts (SPEC §6.3).
pub const DRIFT_THRESHOLD_MS: f64 = 20.0;
/// Correction stops once the remaining error is below this.
const RELEASE_MS: f64 = 2.0;
/// Uncorrected lag above this is a stall and is filled with silence.
const GAP_MS: f64 = 250.0;
/// Lower-envelope window for the lag.
const WINDOW_MS: f64 = 2_000.0;
/// The rate is set so that the current error would be gone in this time.
const SETTLE_S: f64 = 10.0;
/// Max rate change (0.5%, inside what [`Slip`] can sustain at this block).
const MAX_RATE: f64 = 0.005;
/// Samples per slip block (10 ms).
const BLOCK: usize = SAMPLE_RATE as usize / 100;
const SAMPLES_PER_MS: f64 = SAMPLE_RATE as f64 / 1000.0;

pub(crate) struct DriftCorrector {
    slip: Slip<f32>,
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Input samples that went through `slip` (or were flushed).
    consumed: u64,
    /// Samples handed to the writer, including inserted silence.
    produced: u64,
    window: VecDeque<(f64, f64)>,
    first_wall_ms: Option<f64>,
    baseline_ms: Option<f64>,
    estimate_ms: f64,
    engaged: bool,
    silence_samples: u64,
    /// When silence was last inserted, in wall ms.
    last_gap_ms: Option<f64>,
}

impl DriftCorrector {
    pub(crate) fn new() -> Result<Self> {
        let slip = Slip::<f32>::new(BLOCK, 1, FixedAsync::Input)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let out = vec![0.0; slip.output_frames_max()];
        Ok(Self {
            slip,
            pending: Vec::with_capacity(BLOCK),
            out,
            consumed: 0,
            produced: 0,
            window: VecDeque::new(),
            first_wall_ms: None,
            baseline_ms: None,
            estimate_ms: 0.0,
            engaged: false,
            silence_samples: 0,
            last_gap_ms: None,
        })
    }

    /// Timeline correction applied so far, in ms (positive: samples added).
    pub(crate) fn correction_ms(&self) -> f64 {
        (self.produced as f64 - self.consumed as f64) / SAMPLES_PER_MS
    }

    /// Measured drift relative to the start of measurement, in ms.
    pub(crate) fn estimate_ms(&self) -> f64 {
        self.estimate_ms
    }

    /// Silence inserted for device stalls, in samples.
    pub(crate) fn silence_samples(&self) -> u64 {
        self.silence_samples
    }

    /// Feeds captured samples; `emit` receives the corrected stream.
    pub(crate) fn push(
        &mut self,
        mut input: &[f32],
        emit: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        while !input.is_empty() {
            let take = (BLOCK - self.pending.len()).min(input.len());
            self.pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.pending.len() == BLOCK {
                self.process_block(emit)?;
            }
        }
        Ok(())
    }

    /// Passes the buffered remainder (< 10 ms) through unchanged.
    pub(crate) fn flush(&mut self, emit: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
        if !self.pending.is_empty() {
            let n = self.pending.len() as u64;
            emit(&self.pending)?;
            self.consumed += n;
            self.produced += n;
            self.pending.clear();
        }
        Ok(())
    }

    /// Updates the drift estimate from the device's progress and adjusts the
    /// correction. `wall_ms` is the time since the track's first sample and
    /// `delivered` the samples the device has delivered by then. A detected
    /// stall is filled with silence through `emit`.
    pub(crate) fn observe(
        &mut self,
        wall_ms: f64,
        delivered: u64,
        emit: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        let lag = wall_ms - delivered as f64 / SAMPLES_PER_MS;
        self.window.push_back((wall_ms, lag));
        while self
            .window
            .front()
            .is_some_and(|(t, _)| *t < wall_ms - WINDOW_MS)
        {
            self.window.pop_front();
        }
        let first = *self.first_wall_ms.get_or_insert(wall_ms);
        if wall_ms - first < WINDOW_MS {
            return Ok(());
        }
        let envelope = self
            .window
            .iter()
            .map(|(_, l)| *l)
            .fold(f64::INFINITY, f64::min);
        let baseline = *self.baseline_ms.get_or_insert(envelope);
        self.estimate_ms = envelope - baseline;

        let error = self.estimate_ms - self.correction_ms();
        // The envelope reveals a stall in steps as older lags leave the
        // window, so the rest of it is filled with silence too, not slipped.
        let after_gap = self
            .last_gap_ms
            .is_some_and(|t| wall_ms - t <= 2.0 * WINDOW_MS);
        if error > GAP_MS || (after_gap && error > DRIFT_THRESHOLD_MS) {
            self.last_gap_ms = Some(wall_ms);
            self.flush(emit)?;
            let n = (error * SAMPLES_PER_MS).round() as u64;
            self.insert_silence(n, emit)?;
            self.engaged = false;
            self.set_ratio(1.0);
            return Ok(());
        }
        if !self.engaged && error.abs() > DRIFT_THRESHOLD_MS {
            self.engaged = true;
        } else if self.engaged && error.abs() < RELEASE_MS {
            self.engaged = false;
        }
        let ratio = if self.engaged {
            1.0 + (error / 1000.0 / SETTLE_S).clamp(-MAX_RATE, MAX_RATE)
        } else {
            1.0
        };
        self.set_ratio(ratio);
        Ok(())
    }

    fn set_ratio(&mut self, ratio: f64) {
        // Out-of-range ratios are clamped by Slip; the error only says so.
        let _ = self.slip.set_resample_ratio(ratio, false);
    }

    fn insert_silence(
        &mut self,
        mut n: u64,
        emit: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        const ZEROS: [f32; BLOCK] = [0.0; BLOCK];
        self.silence_samples += n;
        while n > 0 {
            let take = usize::try_from(n).unwrap_or(BLOCK).min(BLOCK);
            emit(&ZEROS[..take])?;
            self.produced += take as u64;
            n -= take as u64;
        }
        Ok(())
    }

    fn process_block(&mut self, emit: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
        let input = SequentialSlice::new(&self.pending, 1, BLOCK)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let frames = self.out.len();
        let mut output = SequentialSlice::new_mut(&mut self.out, 1, frames)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: None,
            active_channels_mask: None,
        };
        let (read, written) = self
            .slip
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        self.pending.clear();
        self.consumed += read as u64;
        self.produced += written as u64;
        emit(&self.out[..written])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates a device whose clock is `ppm` slow (positive) or fast,
    /// delivering 10 ms blocks, with optional lateness jitter and a stall.
    struct Sim {
        ppm: f64,
        jitter_ms: f64,
        stall_at_s: Option<(f64, f64)>,
    }

    struct Outcome {
        input: Vec<f32>,
        output: Vec<f32>,
        wall_ms: f64,
        corrector: DriftCorrector,
    }

    impl Sim {
        fn run(&self, secs: f64) -> Outcome {
            let mut c = DriftCorrector::new().unwrap();
            let mut output = Vec::new();
            let mut input = Vec::new();
            let mut emit = |b: &[f32]| {
                output.extend_from_slice(b);
                Ok(())
            };
            let block_ms = 10.0 * (1.0 + self.ppm / 1e6);
            let mut wall = 0.0;
            let mut delivered = 0_u64;
            let mut stalled = false;
            let mut rng = 0x2545_f491_u32;
            for i in 0.. {
                if wall >= secs * 1000.0 {
                    break;
                }
                if let Some((at, len)) = self.stall_at_s
                    && !stalled
                    && wall >= at * 1000.0
                {
                    stalled = true;
                    wall += len * 1000.0;
                }
                let block: Vec<f32> = (0..BLOCK)
                    .map(|k| ((i * BLOCK + k) as f32 * 0.01).sin() * 0.3)
                    .collect();
                wall += block_ms;
                delivered += BLOCK as u64;
                input.extend_from_slice(&block);
                c.push(&block, &mut emit).unwrap();
                if i % 25 == 0 {
                    // xorshift: deterministic lateness in 0..jitter_ms.
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    let late = f64::from(rng % 1000) / 1000.0 * self.jitter_ms;
                    c.observe(wall + late, delivered, &mut emit).unwrap();
                }
            }
            c.flush(&mut emit).unwrap();
            Outcome {
                input,
                output,
                wall_ms: wall,
                corrector: c,
            }
        }
    }

    /// How far the output timeline is from the wall clock, in ms.
    fn timeline_error_ms(o: &Outcome) -> f64 {
        o.output.len() as f64 / SAMPLES_PER_MS - o.wall_ms
    }

    #[test]
    fn steady_clock_passes_audio_through_bit_exact() {
        let o = Sim {
            ppm: 0.0,
            jitter_ms: 0.0,
            stall_at_s: None,
        }
        .run(30.0);
        assert_eq!(o.output, o.input);
        assert_eq!(o.corrector.correction_ms(), 0.0);
    }

    #[test]
    fn delivery_jitter_does_not_trigger_correction() {
        let o = Sim {
            ppm: 0.0,
            jitter_ms: 40.0,
            stall_at_s: None,
        }
        .run(60.0);
        assert_eq!(o.output, o.input, "jitter alone must not alter audio");
    }

    #[test]
    fn slow_device_is_stretched_to_the_wall_clock() {
        // 500 ppm slow: 30 ms behind after 60 s, 60 ms after 120 s.
        let o = Sim {
            ppm: 500.0,
            jitter_ms: 15.0,
            stall_at_s: None,
        }
        .run(120.0);
        let err = timeline_error_ms(&o);
        assert!(
            err.abs() <= DRIFT_THRESHOLD_MS,
            "timeline off by {err:.1} ms"
        );
        assert!(o.corrector.correction_ms() > 30.0);
        assert_eq!(o.corrector.silence_samples(), 0);
    }

    #[test]
    fn fast_device_is_shortened_to_the_wall_clock() {
        let o = Sim {
            ppm: -800.0,
            jitter_ms: 15.0,
            stall_at_s: None,
        }
        .run(120.0);
        let err = timeline_error_ms(&o);
        assert!(
            err.abs() <= DRIFT_THRESHOLD_MS,
            "timeline off by {err:.1} ms"
        );
        assert!(o.corrector.correction_ms() < -50.0);
    }

    #[test]
    fn stall_is_filled_with_silence() {
        let o = Sim {
            ppm: 0.0,
            jitter_ms: 10.0,
            stall_at_s: Some((10.0, 0.6)),
        }
        .run(20.0);
        let silence_ms = o.corrector.silence_samples() as f64 / SAMPLES_PER_MS;
        assert!((550.0..=650.0).contains(&silence_ms), "{silence_ms} ms");
        let err = timeline_error_ms(&o);
        assert!(
            err.abs() <= DRIFT_THRESHOLD_MS,
            "timeline off by {err:.1} ms"
        );
    }
}
