//! Offline chunker (SPEC §6.3, D1): turns a finished track archive into ASR
//! chunks. It runs after recording stops, never in the capture path.
//!
//! `<track>.opus` → decode → 16 kHz → VAD → cut into ~15–30 s chunks at
//! silences (hard cap 30 s) → `chunks/<track>-<index>.wav` (16-bit mono).
//! Chunks without speech are not written, so they are never sent to ASR.
//! Long silences are dropped (up to a short pad around speech) so they don't
//! cost ASR time. Every chunk keeps its position on the recording clock.
//!
//! Memory is bounded by one chunk (≤ 30 s at 16 kHz) whatever the length of
//! the recording. The same path serves normal processing, crash recovery
//! (truncated archives decode up to the last complete page) and
//! re-transcription: existing chunks of the track are replaced.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use tracing::{debug, info};

use crate::resample::{CHUNK_SAMPLE_RATE, Downsampler};
use crate::vad::{VAD_WINDOW, VAD_WINDOW_MS, Vad};
use crate::{OggOpusReader, Result, Track};

/// Tuning for [`chunk_track`]. Durations are rounded to whole VAD windows
/// (32 ms).
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// Don't cut at a silence before a chunk is this long.
    pub min_chunk_ms: u32,
    /// Hard cap: a chunk is cut here even mid-speech.
    pub max_chunk_ms: u32,
    /// A pause at least this long is a cut point once `min_chunk_ms` is reached.
    pub min_silence_ms: u32,
    /// Chunks with less speech than this are skipped.
    pub min_speech_ms: u32,
    /// Silence kept before and after speech at chunk edges. A cut made at a
    /// pause splits it, so each side may keep less than this.
    pub pad_ms: u32,
    /// VAD probability that starts speech. Speech ends below
    /// `threshold - 0.15` (hysteresis, as Silero recommends).
    pub threshold: f32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_chunk_ms: 15_000,
            max_chunk_ms: 30_000,
            min_silence_ms: 300,
            min_speech_ms: 250,
            pad_ms: 300,
            threshold: 0.5,
        }
    }
}

/// One chunk written to disk, ready to become an `asr_jobs` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub track: Track,
    /// 0-based, contiguous over the written chunks of this track.
    pub index: u32,
    pub path: PathBuf,
    /// Position on the recording clock (ms since recording start).
    pub start_ms: i64,
    pub end_ms: i64,
    /// Speech detected inside the chunk.
    pub speech_ms: u32,
}

/// Result of chunking one track.
#[derive(Debug, Clone)]
pub struct ChunkReport {
    pub track: Track,
    pub chunks: Vec<Chunk>,
    /// Decoded duration of the archive.
    pub duration_ms: i64,
    /// Speech found in the whole track, including in skipped chunks.
    pub speech_ms: u32,
    /// Spans that held some speech but less than `min_speech_ms`.
    pub skipped: u32,
    /// The archive had no end-of-stream page (the recorder crashed).
    pub truncated: bool,
}

/// Chunks `opus` (one track's archive) into `out_dir`.
///
/// `start_offset_ms` is where the track's first sample sits on the recording
/// clock (`TrackSummary::start_offset_ms`). Existing chunk files for `track`
/// in `out_dir` are removed first, so the call is safe to repeat.
pub fn chunk_track(
    opus: &Path,
    out_dir: &Path,
    track: Track,
    start_offset_ms: i64,
    vad: &mut dyn Vad,
    config: &ChunkerConfig,
) -> Result<ChunkReport> {
    fs::create_dir_all(out_dir)?;
    remove_chunks(out_dir, track)?;
    vad.reset();

    let mut reader = OggOpusReader::new(BufReader::new(File::open(opus)?))?;
    let mut downsampler = Downsampler::new()?;
    let mut segmenter = Segmenter::new(config);
    let mut writer = ChunkWriter {
        dir: out_dir.to_path_buf(),
        track,
        start_offset_ms,
        min_speech_windows: windows(config.min_speech_ms),
        chunks: Vec::new(),
        skipped: 0,
    };
    let mut detector = SpeechDetector::new(config.threshold);
    let mut window = [0.0_f32; VAD_WINDOW];
    let mut filled = 0;
    let mut samples_16k: u64 = 0;
    let mut speech_windows: u64 = 0;

    let mut on_window =
        |w: &[f32], vad: &mut dyn Vad, seg: &mut Segmenter, wr: &mut ChunkWriter| {
            let mut padded = [0.0_f32; VAD_WINDOW];
            padded[..w.len()].copy_from_slice(w);
            let speech = detector.update(vad.speech_prob(&padded)?);
            speech_windows += u64::from(speech);
            seg.push(w, speech, &mut |span| wr.write(span))
        };

    let mut emit = |mut block: &[f32]| -> Result<()> {
        samples_16k += block.len() as u64;
        while !block.is_empty() {
            let take = (VAD_WINDOW - filled).min(block.len());
            window[filled..filled + take].copy_from_slice(&block[..take]);
            filled += take;
            block = &block[take..];
            if filled == VAD_WINDOW {
                on_window(&window, vad, &mut segmenter, &mut writer)?;
                filled = 0;
            }
        }
        Ok(())
    };
    while let Some(block) = reader.next_block()? {
        downsampler.push(block, &mut emit)?;
    }
    downsampler.finish(&mut emit)?;
    if filled > 0 {
        on_window(&window[..filled], vad, &mut segmenter, &mut writer)?;
    }
    segmenter.finish(&mut |span| writer.write(span))?;

    let report = ChunkReport {
        track,
        chunks: writer.chunks,
        duration_ms: samples_to_ms(samples_16k),
        speech_ms: u32::try_from(speech_windows * u64::from(VAD_WINDOW_MS)).unwrap_or(u32::MAX),
        skipped: writer.skipped,
        truncated: reader.is_truncated(),
    };
    info!(
        track = track.as_str(),
        chunks = report.chunks.len(),
        skipped = report.skipped,
        duration_ms = report.duration_ms,
        speech_ms = report.speech_ms,
        truncated = report.truncated,
        "track chunked"
    );
    Ok(report)
}

/// Speech/silence decision with hysteresis over VAD probabilities.
struct SpeechDetector {
    on: f32,
    off: f32,
    speaking: bool,
}

impl SpeechDetector {
    fn new(threshold: f32) -> Self {
        Self {
            on: threshold,
            off: (threshold - 0.15).max(0.01),
            speaking: false,
        }
    }

    fn update(&mut self, prob: f32) -> bool {
        if prob >= self.on {
            self.speaking = true;
        } else if prob < self.off {
            self.speaking = false;
        }
        self.speaking
    }
}

/// A run of windows to write as one chunk.
struct Span<'a> {
    /// Absolute index (in VAD windows) of the first sample.
    start_window: u64,
    audio: &'a [f32],
    speech_windows: usize,
}

/// Decides cut points from per-window speech flags. Pure logic, no I/O.
struct Segmenter {
    min_w: usize,
    max_w: usize,
    min_silence_w: usize,
    pad_w: usize,
    audio: Vec<f32>,
    flags: Vec<bool>,
    start_window: u64,
}

impl Segmenter {
    fn new(c: &ChunkerConfig) -> Self {
        let max_w = windows(c.max_chunk_ms).max(1);
        Self {
            min_w: windows(c.min_chunk_ms).min(max_w),
            max_w,
            min_silence_w: windows(c.min_silence_ms).max(1),
            pad_w: windows(c.pad_ms),
            audio: Vec::with_capacity((max_w + 1) * VAD_WINDOW),
            flags: Vec::with_capacity(max_w + 1),
            start_window: 0,
        }
    }

    /// Adds one window (`VAD_WINDOW` samples; fewer only for the last one).
    fn push(
        &mut self,
        window: &[f32],
        speech: bool,
        out: &mut dyn FnMut(Span<'_>) -> Result<()>,
    ) -> Result<()> {
        self.audio.extend_from_slice(window);
        self.flags.push(speech);
        self.trim_leading_silence();

        let n = self.flags.len();
        let trailing_silence = self.flags.iter().rev().take_while(|s| !**s).count();
        if n >= self.min_w && trailing_silence >= self.min_silence_w && trailing_silence < n {
            // Cut in the middle of the pause.
            self.cut(n - trailing_silence / 2, out)?;
        } else if n >= self.max_w {
            let cut = self.best_cut_before_cap();
            self.cut(cut, out)?;
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut dyn FnMut(Span<'_>) -> Result<()>) -> Result<()> {
        if !self.flags.is_empty() {
            self.cut(self.flags.len(), out)?;
        }
        Ok(())
    }

    /// Until speech starts, keep only `pad_w` windows of lead-in.
    fn trim_leading_silence(&mut self) {
        if self.flags.len() > self.pad_w && !self.flags.iter().any(|s| *s) {
            let drop = self.flags.len() - self.pad_w;
            self.flags.drain(..drop);
            self.audio.drain(..drop * VAD_WINDOW);
            self.start_window += drop as u64;
        }
    }

    /// At the cap: the middle of the longest pause in `[min_w, max_w)`, or the
    /// cap itself when there is none (continuous speech).
    fn best_cut_before_cap(&self) -> usize {
        let (mut best_len, mut best_end, mut run) = (0, 0, 0);
        for i in self.min_w..self.max_w {
            run = if self.flags[i] { 0 } else { run + 1 };
            if run > best_len {
                (best_len, best_end) = (run, i + 1);
            }
        }
        if best_len == 0 {
            self.max_w
        } else {
            best_end - best_len / 2
        }
    }

    /// Emits windows `[0, cut)` (trailing silence trimmed to the pad) and
    /// keeps the rest for the next chunk.
    fn cut(&mut self, cut: usize, out: &mut dyn FnMut(Span<'_>) -> Result<()>) -> Result<()> {
        let speech_windows = self.flags[..cut].iter().filter(|s| **s).count();
        if speech_windows > 0 {
            let last_speech = self.flags[..cut].iter().rposition(|s| *s).unwrap_or(0);
            let end_w = (last_speech + 1 + self.pad_w).min(cut);
            let end = (end_w * VAD_WINDOW).min(self.audio.len());
            out(Span {
                start_window: self.start_window,
                audio: &self.audio[..end],
                speech_windows,
            })?;
        }
        let cut_samples = (cut * VAD_WINDOW).min(self.audio.len());
        self.audio.drain(..cut_samples);
        self.flags.drain(..cut);
        self.start_window += cut as u64;
        self.trim_leading_silence();
        Ok(())
    }
}

struct ChunkWriter {
    dir: PathBuf,
    track: Track,
    start_offset_ms: i64,
    min_speech_windows: usize,
    chunks: Vec<Chunk>,
    skipped: u32,
}

impl ChunkWriter {
    fn write(&mut self, span: Span<'_>) -> Result<()> {
        let start_ms = self.start_offset_ms + span.start_window as i64 * i64::from(VAD_WINDOW_MS);
        let end_ms = start_ms + samples_to_ms(span.audio.len() as u64);
        if span.speech_windows < self.min_speech_windows.max(1) {
            debug!(start_ms, end_ms, "skipping chunk with too little speech");
            self.skipped += 1;
            return Ok(());
        }
        let index = u32::try_from(self.chunks.len()).unwrap_or(u32::MAX);
        let path = self.dir.join(chunk_file_name(self.track, index));
        let part = path.with_extension("wav.part");
        let spec = WavSpec {
            channels: 1,
            sample_rate: CHUNK_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut wav = WavWriter::create(&part, spec)?;
        for &s in span.audio {
            // Saturating float → int cast after scaling.
            wav.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
        }
        wav.finalize()?;
        fs::rename(&part, &path)?;
        self.chunks.push(Chunk {
            track: self.track,
            index,
            path,
            start_ms,
            end_ms,
            speech_ms: u32::try_from(span.speech_windows).unwrap_or(u32::MAX) * VAD_WINDOW_MS,
        });
        Ok(())
    }
}

/// `chunks/<track>-<index>.wav` (SPEC §7.1); the index is zero-padded so
/// files sort in time order.
pub fn chunk_file_name(track: Track, index: u32) -> String {
    format!("{}-{index:04}.wav", track.as_str())
}

/// Removes earlier chunks (and half-written ones) of `track` from `dir`.
fn remove_chunks(dir: &Path, track: Track) -> Result<()> {
    let prefix = format!("{}-", track.as_str());
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && (name.ends_with(".wav") || name.ends_with(".wav.part")) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn windows(ms: u32) -> usize {
    (ms / VAD_WINDOW_MS) as usize
}

fn samples_to_ms(samples: u64) -> i64 {
    i64::try_from(samples * 1000 / u64::from(CHUNK_SAMPLE_RATE)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the segmenter over flags; returns (start_window, len_windows, speech).
    fn segment(flags: &[bool], cfg: &ChunkerConfig) -> Vec<(u64, usize, usize)> {
        let mut seg = Segmenter::new(cfg);
        let mut spans = Vec::new();
        let window = [0.0_f32; VAD_WINDOW];
        let mut out = |s: Span<'_>| {
            spans.push((s.start_window, s.audio.len() / VAD_WINDOW, s.speech_windows));
            Ok(())
        };
        for &f in flags {
            seg.push(&window, f, &mut out).unwrap();
        }
        seg.finish(&mut out).unwrap();
        spans
    }

    fn run(pattern: &[(bool, usize)]) -> Vec<bool> {
        pattern
            .iter()
            .flat_map(|&(s, n)| std::iter::repeat_n(s, n))
            .collect()
    }

    // With the default config: min 468 windows (≈15 s), max 937 (≈30 s),
    // pause 9 windows, pad 9 windows.

    #[test]
    fn silence_only_produces_nothing() {
        assert!(segment(&run(&[(false, 3000)]), &ChunkerConfig::default()).is_empty());
    }

    #[test]
    fn continuous_speech_is_hard_capped() {
        let spans = segment(&run(&[(true, 2000)]), &ChunkerConfig::default());
        let lens: Vec<usize> = spans.iter().map(|s| s.1).collect();
        assert_eq!(lens, vec![937, 937, 126]);
        assert_eq!(spans[1].0, 937);
    }

    #[test]
    fn cuts_at_first_pause_after_minimum() {
        // Pause before the minimum is ignored; the one after it is used.
        let flags = run(&[
            (true, 300),
            (false, 20),
            (true, 300),
            (false, 20),
            (true, 300),
        ]);
        let spans = segment(&flags, &ChunkerConfig::default());
        assert_eq!(spans.len(), 2);
        // The cut happens once the pause reaches 9 windows (at 629), in its
        // middle: 629 - 9/2 = 625. The pad can't extend past the cut.
        assert_eq!(spans[0], (0, 625, 600));
        // The next chunk starts one pad (9 windows) before speech resumes at 640.
        assert_eq!(spans[1].0, 631);
    }

    #[test]
    fn long_leading_and_trailing_silence_is_dropped() {
        let flags = run(&[(false, 1000), (true, 100), (false, 1000)]);
        let spans = segment(&flags, &ChunkerConfig::default());
        // Starts 9 windows (pad) before the speech, ends 9 after it.
        assert_eq!(spans, vec![(991, 118, 100)]);
    }

    #[test]
    fn cap_prefers_longest_short_pause() {
        // Pauses shorter than the 9-window minimum, the longest at ~700.
        let flags = run(&[(true, 600), (false, 3), (true, 97), (false, 6), (true, 300)]);
        let spans = segment(&flags, &ChunkerConfig::default());
        // Longest pause is windows 700..706; cut in its middle (703).
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans[0].1, 703);
        assert_eq!(spans[1].0, 703);
    }

    #[test]
    fn hysteresis_keeps_speech_through_dips() {
        let mut d = SpeechDetector::new(0.5);
        let decisions: Vec<bool> = [0.2, 0.6, 0.4, 0.36, 0.3, 0.7]
            .iter()
            .map(|&p| d.update(p))
            .collect();
        assert_eq!(decisions, vec![false, true, true, true, false, true]);
    }
}
