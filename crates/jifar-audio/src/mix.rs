//! Renders `mixed.opus` from the track archives on stop (SPEC §6.3), for
//! playback and export, plus a waveform overview for the UI.
//!
//! Tracks are aligned by where their first sample sits on the recording
//! clock (`start_offset_ms`), summed and clipped to full scale. Streaming:
//! memory stays constant however long the meeting is. Written to
//! `mixed.opus.part` and renamed, so a crash never leaves a half file.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::opus_writer::{DurableFile, OggOpusWriter};
use crate::{OggOpusReader, Result, SAMPLE_RATE};

/// Samples per waveform window before downsampling (100 ms).
const PEAK_WINDOW: usize = SAMPLE_RATE as usize / 10;
const BLOCK: usize = SAMPLE_RATE as usize / 10;

/// One track to mix: its archive and its start on the recording clock.
#[derive(Debug, Clone)]
pub struct MixInput {
    pub path: PathBuf,
    pub start_offset_ms: i64,
}

#[derive(Debug, Clone)]
pub struct MixReport {
    pub path: PathBuf,
    pub duration_ms: u64,
    /// Waveform overview: `bars` peak values in 0..=1, oldest first.
    pub peaks: Vec<f32>,
}

struct Lane {
    reader: OggOpusReader<BufReader<File>>,
    /// Leading silence still to emit (aligns later-starting tracks).
    lead: usize,
    queue: VecDeque<f32>,
    done: bool,
}

impl Lane {
    /// Next `n` samples (zero-padded once the track ends). False if the
    /// track has nothing left at all.
    fn take(&mut self, out: &mut [f32]) -> Result<bool> {
        let mut i = 0;
        while i < out.len() && self.lead > 0 {
            out[i] = 0.0;
            i += 1;
            self.lead -= 1;
        }
        while i < out.len() {
            if let Some(s) = self.queue.pop_front() {
                out[i] = s;
                i += 1;
                continue;
            }
            if self.done {
                break;
            }
            match self.reader.next_block()? {
                Some(block) => self.queue.extend(block.iter().copied()),
                None => self.done = true,
            }
        }
        let produced = i > 0;
        out[i..].fill(0.0);
        Ok(produced || !self.done)
    }
}

/// Mixes `inputs` into `out` (an `.opus` path) and returns a waveform of
/// `bars` peaks. Tracks that can't be opened are skipped; fails only if
/// none can be read.
pub fn render_mixed(inputs: &[MixInput], out: &Path, bars: usize) -> Result<MixReport> {
    let base = inputs.iter().map(|i| i.start_offset_ms).min().unwrap_or(0);
    let mut lanes = Vec::new();
    for input in inputs {
        let Ok(file) = File::open(&input.path) else {
            continue;
        };
        let Ok(reader) = OggOpusReader::new(BufReader::new(file)) else {
            continue;
        };
        let lead_ms = u64::try_from(input.start_offset_ms - base).unwrap_or(0);
        lanes.push(Lane {
            reader,
            lead: usize::try_from(lead_ms * u64::from(SAMPLE_RATE) / 1000).unwrap_or(0),
            queue: VecDeque::new(),
            done: false,
        });
    }
    if lanes.is_empty() {
        return Err(crate::AudioError::InvalidStream(
            "no track could be read".into(),
        ));
    }

    let part = out.with_extension("opus.part");
    let mut writer = OggOpusWriter::new(DurableFile::create(&part)?, 0x6d69_7864)?;
    let mut mix = vec![0.0_f32; BLOCK];
    let mut lane_buf = vec![0.0_f32; BLOCK];
    let mut windows = Vec::new();
    let (mut peak, mut in_window) = (0.0_f32, 0_usize);
    loop {
        mix.fill(0.0);
        let mut any = false;
        let mut longest = 0;
        for lane in &mut lanes {
            if lane.take(&mut lane_buf)? {
                any = true;
                let real = lane_buf
                    .iter()
                    .rposition(|s| *s != 0.0)
                    .map_or(0, |p| p + 1);
                longest = longest.max(if lane.done && lane.queue.is_empty() {
                    real
                } else {
                    BLOCK
                });
                for (m, s) in mix.iter_mut().zip(&lane_buf) {
                    *m += s;
                }
            }
        }
        if !any {
            break;
        }
        let block = &mut mix[..longest.max(1)];
        for s in block.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
            peak = peak.max(s.abs());
            in_window += 1;
            if in_window == PEAK_WINDOW {
                windows.push(peak);
                (peak, in_window) = (0.0, 0);
            }
        }
        writer.write_samples(block)?;
        if lanes
            .iter()
            .all(|l| l.done && l.queue.is_empty() && l.lead == 0)
        {
            break;
        }
    }
    if in_window > 0 {
        windows.push(peak);
    }
    let samples = writer.samples_written();
    writer.finish()?.close()?;
    fs::rename(&part, out)?;
    Ok(MixReport {
        path: out.to_owned(),
        duration_ms: samples * 1000 / u64::from(SAMPLE_RATE),
        peaks: downsample(&windows, bars),
    })
}

/// Max-pools `windows` into `bars` values, normalized so the loudest is 1.
fn downsample(windows: &[f32], bars: usize) -> Vec<f32> {
    if windows.is_empty() || bars == 0 {
        return Vec::new();
    }
    let bars = bars.min(windows.len());
    let mut out: Vec<f32> = (0..bars)
        .map(|b| {
            let lo = b * windows.len() / bars;
            let hi = ((b + 1) * windows.len() / bars).max(lo + 1);
            windows[lo..hi].iter().copied().fold(0.0, f32::max)
        })
        .collect();
    let max = out.iter().copied().fold(0.0, f32::max);
    if max > 0.0 {
        for v in &mut out {
            *v /= max;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, samples: &[f32]) {
        let mut w = OggOpusWriter::new(DurableFile::create(path).unwrap(), 1).unwrap();
        w.write_samples(samples).unwrap();
        w.finish().unwrap().close().unwrap();
    }

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| amp * (i as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn mixes_aligned_tracks_and_reports_a_waveform() {
        let dir = tempfile::tempdir().unwrap();
        let (mic, sys) = (dir.path().join("mic.opus"), dir.path().join("system.opus"));
        // Mic: 2 s of tone then 1 s quiet. System starts 500 ms later, 1 s.
        let mut m = tone(2.0, 0.4);
        m.extend(vec![0.0; SAMPLE_RATE as usize]);
        write(&mic, &m);
        write(&sys, &tone(1.0, 0.2));
        let out = dir.path().join("mixed.opus");
        let report = render_mixed(
            &[
                MixInput {
                    path: mic,
                    start_offset_ms: 10,
                },
                MixInput {
                    path: sys,
                    start_offset_ms: 510,
                },
            ],
            &out,
            30,
        )
        .unwrap();
        assert!(
            (2_950..=3_050).contains(&report.duration_ms),
            "{}",
            report.duration_ms
        );
        assert!(out.is_file() && !out.with_extension("opus.part").exists());
        assert_eq!(report.peaks.len(), 30);
        // Loud first two-thirds, quiet last third.
        assert!(
            report.peaks[..18].iter().all(|p| *p > 0.5),
            "{:?}",
            report.peaks
        );
        assert!(
            report.peaks[22..].iter().all(|p| *p < 0.05),
            "{:?}",
            report.peaks
        );
        let mut r = OggOpusReader::new(File::open(&out).unwrap()).unwrap();
        assert!(!r.read_to_end().unwrap().is_empty());
    }

    #[test]
    fn missing_tracks_are_skipped_but_none_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mic = dir.path().join("mic.opus");
        write(&mic, &tone(0.5, 0.3));
        let out = dir.path().join("mixed.opus");
        let gone = MixInput {
            path: dir.path().join("nope.opus"),
            start_offset_ms: 0,
        };
        let ok = render_mixed(
            &[
                MixInput {
                    path: mic,
                    start_offset_ms: 0,
                },
                gone.clone(),
            ],
            &out,
            10,
        );
        assert!(ok.is_ok());
        assert!(render_mixed(&[gone], &out, 10).is_err());
    }
}
