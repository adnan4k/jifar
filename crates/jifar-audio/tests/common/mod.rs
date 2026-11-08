#![allow(dead_code)]

use std::f32::consts::TAU;

use jifar_audio::SAMPLE_RATE;

/// `secs` of a sine at `freq` Hz, amplitude 0.5.
pub fn sine(freq: f32, secs: f32) -> Vec<f32> {
    let n = (secs * SAMPLE_RATE as f32) as usize;
    (0..n)
        .map(|i| 0.5 * (TAU * freq * i as f32 / SAMPLE_RATE as f32).sin())
        .collect()
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// `secs` of a linear chirp from 200 Hz to 2 kHz, amplitude 0.5. Unlike a
/// sine it has a unique cross-correlation peak, so it can prove alignment.
pub fn chirp(secs: f32) -> Vec<f32> {
    let n = (secs * SAMPLE_RATE as f32) as usize;
    let (f0, f1) = (200.0_f32, 2000.0_f32);
    let k = (f1 - f0) / secs;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.5 * (TAU * (f0 * t + 0.5 * k * t * t)).sin()
        })
        .collect()
}

/// Lag (in samples) in `-max_lag..=max_lag` that best aligns `decoded` with
/// `reference`. 0 means the decoder output is sample-aligned with the input.
pub fn best_lag(reference: &[f32], decoded: &[f32], max_lag: isize) -> isize {
    let n = reference.len().min(decoded.len()) as isize;
    (-max_lag..=max_lag)
        .max_by(|&a, &b| corr(reference, decoded, a, n).total_cmp(&corr(reference, decoded, b, n)))
        .unwrap_or(0)
}

fn corr(reference: &[f32], decoded: &[f32], lag: isize, n: isize) -> f32 {
    (0..n)
        .filter_map(|i| {
            let j = i + lag;
            (0..n)
                .contains(&j)
                .then(|| reference[i as usize] * decoded[j as usize])
        })
        .sum()
}

/// Asserts `decoded` is aligned with `reference` and at a similar level.
///
/// Opus itself shifts phase by a few samples (~0.05 ms), so ±5 samples is
/// "aligned"; a pre-skip bug would show up as an offset of ~300 samples. Only
/// a 0.25 s window is correlated to keep debug-build tests fast.
pub fn assert_aligned(reference: &[f32], decoded: &[f32]) {
    let n = reference.len().min(decoded.len());
    let win = (SAMPLE_RATE as usize / 4).min(n);
    let start = (n - win) / 2;
    let lag = best_lag(
        &reference[start..start + win],
        &decoded[start..start + win],
        1200,
    );
    assert!(lag.abs() <= 5, "decoded audio is offset by {lag} samples");
    let ratio = rms(&decoded[..n]) / rms(&reference[..n]);
    assert!(
        (0.7..1.3).contains(&ratio),
        "level changed: ratio {ratio:.2}"
    );
}
