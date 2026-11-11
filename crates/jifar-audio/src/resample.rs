//! Streaming 48 kHz → 16 kHz mono resampler for the offline chunker.
//!
//! Wraps rubato's FFT resampler (the ratio is fixed), hides its startup delay
//! and flushes the tail, so `n` input samples always produce exactly
//! `ceil(n / 3)` output samples, aligned with the input. Memory is constant.

use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::{AudioError, Result, SAMPLE_RATE};

/// Output rate of the chunker and of every ASR chunk (SPEC §6.3).
pub const CHUNK_SAMPLE_RATE: u32 = 16_000;

/// Input frames per resampler call (100 ms at 48 kHz).
const BLOCK: usize = SAMPLE_RATE as usize / 10;

pub(crate) struct Downsampler {
    inner: Fft<f32>,
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output samples still to drop to cancel the resampler delay.
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl Downsampler {
    pub(crate) fn new() -> Result<Self> {
        let inner = Fft::<f32>::new(
            SAMPLE_RATE as usize,
            CHUNK_SAMPLE_RATE as usize,
            BLOCK,
            1,
            FixedSync::Input,
        )
        .map_err(|e| AudioError::Resample(e.to_string()))?;
        let delay_left = inner.output_delay();
        let out = vec![0.0; inner.output_frames_max()];
        Ok(Self {
            inner,
            pending: Vec::with_capacity(BLOCK),
            out,
            delay_left,
            in_total: 0,
            out_total: 0,
        })
    }

    /// Feeds 48 kHz samples; calls `emit` with each block of 16 kHz output.
    pub(crate) fn push(
        &mut self,
        mut input: &[f32],
        emit: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        self.in_total += input.len() as u64;
        while !input.is_empty() {
            let need = BLOCK - self.pending.len();
            let take = need.min(input.len());
            self.pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.pending.len() == BLOCK {
                self.process(BLOCK, emit)?;
            }
        }
        Ok(())
    }

    /// Flushes buffered input and the resampler's delay line.
    pub(crate) fn finish(&mut self, emit: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
        let target = self.in_total.div_ceil(3);
        let partial = self.pending.len();
        self.pending.resize(BLOCK, 0.0);
        self.process(partial, emit)?;
        // Silence pushes the delayed tail out; `process` stops at `target`.
        while self.out_total < target {
            self.pending.clear();
            self.pending.resize(BLOCK, 0.0);
            self.process(0, emit)?;
        }
        Ok(())
    }

    fn process(&mut self, valid: usize, emit: &mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()> {
        let target = self.in_total.div_ceil(3);
        let input = SequentialSlice::new(&self.pending, 1, BLOCK)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let frames = self.out.len();
        let mut output = SequentialSlice::new_mut(&mut self.out, 1, frames)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: (valid < BLOCK).then_some(valid),
            active_channels_mask: None,
        };
        let (_, produced) = self
            .inner
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        self.pending.clear();

        let skip = self.delay_left.min(produced);
        self.delay_left -= skip;
        let left = usize::try_from(target.saturating_sub(self.out_total)).unwrap_or(usize::MAX);
        let end = produced.min(skip + left);
        if skip < end {
            self.out_total += (end - skip) as u64;
            emit(&self.out[skip..end])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn run(input: &[f32], step: usize) -> Vec<f32> {
        let mut ds = Downsampler::new().unwrap();
        let mut out = Vec::new();
        let mut emit = |b: &[f32]| {
            out.extend_from_slice(b);
            Ok(())
        };
        for block in input.chunks(step) {
            ds.push(block, &mut emit).unwrap();
        }
        ds.finish(&mut emit).unwrap();
        out
    }

    #[test]
    fn output_length_is_exact_for_any_input_length() {
        for n in [0, 1, 2, 3, 959, 4800, 4801, 48_000, 50_123] {
            let out = run(&vec![0.1; n], 777);
            assert_eq!(out.len(), n.div_ceil(3), "input {n}");
        }
    }

    #[test]
    fn tone_is_preserved_and_aligned() {
        // 440 Hz tone, 1 s. After resampling, sample k at 16 kHz must match
        // the analytic tone at t = k / 16000: the resampler delay is removed.
        // The FFT resampler leaves ~1/3 output sample (20 µs) of phase
        // offset; 0.045 allows < 0.5 sample, while a delay bug is ≥ 1 sample.
        let input: Vec<f32> = (0..48_000)
            .map(|i| 0.5 * (TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = run(&input, 1000);
        let max_err = (2000..14_000)
            .map(|k| {
                let expected = 0.5 * (TAU * 440.0 * k as f32 / 16_000.0).sin();
                (out[k] - expected).abs()
            })
            .fold(0.0_f32, f32::max);
        assert!(max_err < 0.045, "max error {max_err}");
    }
}
