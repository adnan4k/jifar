// Test helpers outside #[test] fns are not covered by allow-unwrap-in-tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;

use common::sine;
use jifar_audio::{
    CHUNK_SAMPLE_RATE, ChunkerConfig, EnergyVad, OggOpusWriter, SAMPLE_RATE, SileroVad, Track,
    VAD_WINDOW, Vad, chunk_track,
};

/// "Speech" for [`EnergyVad`]: a tone loud enough to pass its threshold.
fn talk(secs: f32) -> Vec<f32> {
    sine(220.0, secs)
}

fn quiet(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * SAMPLE_RATE as f32) as usize]
}

fn write_opus(path: &Path, parts: &[Vec<f32>]) {
    let mut w = OggOpusWriter::new(Vec::new(), 1).unwrap();
    for p in parts {
        w.write_samples(p).unwrap();
    }
    fs::write(path, w.finish().unwrap()).unwrap();
}

fn wav_len_ms(path: &Path) -> i64 {
    let r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    assert_eq!(spec.sample_rate, CHUNK_SAMPLE_RATE);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 16);
    i64::from(r.duration()) * 1000 / i64::from(CHUNK_SAMPLE_RATE)
}

/// Asserts `actual` is within `tol` ms of `expected`.
#[track_caller]
fn near(actual: i64, expected: i64, tol: i64) {
    assert!(
        (actual - expected).abs() <= tol,
        "{actual} ms is not within {tol} ms of {expected} ms"
    );
}

#[test]
fn chunks_a_meeting_like_track() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    // 0–5 s quiet · 5–25 s talk · 25–26 s pause · 26–46 s talk ·
    // 46–106 s quiet · 106–110 s talk.
    write_opus(
        &opus,
        &[
            quiet(5.0),
            talk(20.0),
            quiet(1.0),
            talk(20.0),
            quiet(60.0),
            talk(4.0),
        ],
    );
    let chunks_dir = dir.path().join("chunks");
    let offset = 1_234;
    let report = chunk_track(
        &opus,
        &chunks_dir,
        Track::Mic,
        offset,
        &mut EnergyVad::default(),
        &ChunkerConfig::default(),
    )
    .unwrap();

    assert!(!report.truncated);
    near(report.duration_ms, 110_000, 5);
    near(i64::from(report.speech_ms), 44_000, 300);
    assert_eq!(report.skipped, 0);

    let c = &report.chunks;
    assert_eq!(c.len(), 3, "{c:#?}");
    // Leading quiet is dropped down to the 300 ms pad.
    near(c[0].start_ms, offset + 5_000 - 300, 40);
    // Cut in the middle of the 1 s pause (after the 15 s minimum).
    let mid_pause = offset + 25_500;
    assert!((c[0].end_ms - mid_pause).abs() < 400, "{:?}", c[0]);
    // Second chunk: rest of the pause and 20 s of talk. It is past the
    // minimum, so it is cut in the middle of the first 300 ms of quiet.
    near(c[1].end_ms, offset + 46_000 + 150, 70);
    // The minute of silence isn't sent; the last 4 s are.
    near(c[2].start_ms, offset + 106_000 - 300, 40);
    near(c[2].end_ms, offset + 110_000, 40);

    for (i, chunk) in c.iter().enumerate() {
        assert_eq!(chunk.index as usize, i);
        assert_eq!(chunk.track, Track::Mic);
        assert_eq!(
            chunk.path.file_name().unwrap().to_str().unwrap(),
            format!("mic-{i:04}.wav")
        );
        assert!(chunk.end_ms - chunk.start_ms <= 30_000, "{chunk:?}");
        near(wav_len_ms(&chunk.path), chunk.end_ms - chunk.start_ms, 1);
    }
    // No temporary files left behind.
    assert_eq!(fs::read_dir(&chunks_dir).unwrap().count(), 3);
}

#[test]
fn continuous_speech_is_capped_at_30_s() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("system.opus");
    write_opus(&opus, &[talk(70.0)]);
    let report = chunk_track(
        &opus,
        dir.path(),
        Track::System,
        0,
        &mut EnergyVad::default(),
        &ChunkerConfig::default(),
    )
    .unwrap();
    let lens: Vec<i64> = report
        .chunks
        .iter()
        .map(|c| c.end_ms - c.start_ms)
        .collect();
    assert_eq!(lens.len(), 3, "{lens:?}");
    assert!(lens.iter().all(|l| *l <= 30_000), "{lens:?}");
    // Chunks are back to back: nothing lost at the cuts.
    for pair in report.chunks.windows(2) {
        assert_eq!(pair[0].end_ms, pair[1].start_ms);
    }
    near(report.chunks[2].end_ms, 70_000, 40);
    assert!(report.chunks[0].path.ends_with("system-0000.wav"));
}

#[test]
fn silent_track_produces_no_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    write_opus(&opus, &[quiet(45.0)]);
    let report = chunk_track(
        &opus,
        dir.path(),
        Track::Mic,
        0,
        &mut EnergyVad::default(),
        &ChunkerConfig::default(),
    )
    .unwrap();
    assert!(report.chunks.is_empty());
    assert_eq!(report.speech_ms, 0);
}

#[test]
fn blips_below_minimum_speech_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    // A 100 ms click, then quiet: less than the 250 ms minimum.
    write_opus(&opus, &[quiet(2.0), talk(0.1), quiet(20.0)]);
    let report = chunk_track(
        &opus,
        dir.path(),
        Track::Mic,
        0,
        &mut EnergyVad::default(),
        &ChunkerConfig::default(),
    )
    .unwrap();
    assert!(report.chunks.is_empty(), "{:?}", report.chunks);
    assert_eq!(report.skipped, 1);
}

#[test]
fn rechunking_replaces_previous_chunks_of_that_track_only() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    let chunks = dir.path().join("chunks");
    fs::create_dir_all(&chunks).unwrap();
    // Left over from an earlier, longer run, plus a crash mid-write.
    fs::write(chunks.join("mic-0009.wav"), b"stale").unwrap();
    fs::write(chunks.join("mic-0010.wav.part"), b"torn").unwrap();
    fs::write(chunks.join("system-0000.wav"), b"other track").unwrap();

    write_opus(&opus, &[talk(3.0)]);
    let cfg = ChunkerConfig::default();
    for _ in 0..2 {
        let report = chunk_track(
            &opus,
            &chunks,
            Track::Mic,
            0,
            &mut EnergyVad::default(),
            &cfg,
        )
        .unwrap();
        assert_eq!(report.chunks.len(), 1);
    }
    let mut names: Vec<String> = fs::read_dir(&chunks)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["mic-0000.wav", "system-0000.wav"]);
}

#[test]
fn crashed_recording_is_chunked_up_to_the_last_page() {
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    // Never finished (no end-of-stream page) and the last page torn.
    let mut w = OggOpusWriter::new(Vec::new(), 3).unwrap();
    w.write_samples(&talk(10.0)).unwrap();
    let mut bytes = w.get_ref().clone();
    bytes.truncate(bytes.len() - 100);
    fs::write(&opus, bytes).unwrap();

    let report = chunk_track(
        &opus,
        dir.path(),
        Track::Mic,
        0,
        &mut EnergyVad::default(),
        &ChunkerConfig::default(),
    )
    .unwrap();
    assert!(report.truncated);
    assert_eq!(report.chunks.len(), 1);
    // Most of the audio survives (pages are ~1 s).
    assert!(report.duration_ms > 7_000, "{}", report.duration_ms);
}

/// The Silero model needs ONNX Runtime (`ORT_DYLIB_PATH`). Locally the test
/// is skipped without it; CI sets `JIFAR_REQUIRE_ORT=1` so it can't be.
fn silero() -> Option<SileroVad> {
    match SileroVad::new() {
        Ok(vad) => Some(vad),
        Err(e) if std::env::var_os("JIFAR_REQUIRE_ORT").is_none() => {
            eprintln!("skipping Silero test: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn silero_scores_silence_and_tones_as_non_speech() {
    let Some(mut vad) = silero() else { return };
    let silence = [0.0_f32; VAD_WINDOW];
    for _ in 0..20 {
        let p = vad.speech_prob(&silence).unwrap();
        assert!((0.0..0.2).contains(&p), "silence scored {p}");
    }
    vad.reset();
    let tone: Vec<f32> = (0..VAD_WINDOW * 30)
        .map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / 16_000.0).sin())
        .collect();
    let mut max = 0.0_f32;
    for w in tone.as_chunks::<VAD_WINDOW>().0 {
        max = max.max(vad.speech_prob(w).unwrap());
    }
    assert!(max < 0.5, "steady tone scored {max}");
}

#[test]
fn silero_chunker_end_to_end_on_silence() {
    let Some(mut vad) = silero() else { return };
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("mic.opus");
    write_opus(&opus, &[quiet(10.0)]);
    let report = chunk_track(
        &opus,
        dir.path(),
        Track::Mic,
        0,
        &mut vad,
        &ChunkerConfig::default(),
    )
    .unwrap();
    assert!(report.chunks.is_empty());
    near(report.duration_ms, 10_000, 5);
}
