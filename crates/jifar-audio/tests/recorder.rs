// Test helpers outside #[test] fns are not covered by allow-unwrap-in-tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;
use std::thread;
use std::time::{Duration, Instant};

use common::{assert_aligned, chirp, sine};
use jifar_audio::{AudioError, FakeSource, OggOpusReader, Pacing, SAMPLE_RATE, TrackRecorder};

fn wait_until_finished(rec: &TrackRecorder, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !rec.is_source_finished() {
        assert!(Instant::now() < deadline, "source did not finish in time");
        thread::sleep(Duration::from_millis(5));
    }
}

fn decode(path: &std::path::Path) -> (Vec<f32>, bool) {
    let mut r = OggOpusReader::new(File::open(path).unwrap()).unwrap();
    let samples = r.read_to_end().unwrap();
    (samples, r.is_truncated())
}

#[test]
fn records_fake_source_to_opus_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mic.opus");
    let input = chirp(3.0);
    let source = FakeSource::from_samples("sine", input.clone(), Pacing::Lossless);

    let rec = TrackRecorder::start(Box::new(source), &path).unwrap();
    wait_until_finished(&rec, Duration::from_secs(10));
    let stats = rec.stop().unwrap();

    assert_eq!(stats.samples, input.len() as u64);
    assert_eq!(stats.duration_ms, 3000);
    assert_eq!(stats.overrun_samples, 0);

    let (decoded, truncated) = decode(&path);
    assert!(!truncated);
    assert_eq!(decoded.len(), input.len());
    assert_aligned(&input, &decoded);
}

#[test]
fn realtime_source_has_no_overruns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("system.opus");
    let input = sine(880.0, 0.6);
    let source = FakeSource::from_samples("sine", input.clone(), Pacing::Realtime);

    let started = Instant::now();
    let rec = TrackRecorder::start(Box::new(source), &path).unwrap();
    wait_until_finished(&rec, Duration::from_secs(5));
    let stats = rec.stop().unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(550),
        "not paced in real time"
    );
    assert_eq!(stats.overrun_samples, 0);
    assert_eq!(stats.samples, input.len() as u64);
}

#[test]
fn tiny_ring_counts_overruns_instead_of_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mic.opus");
    // 10 ms blocks into a 100-sample ring: most of each block must be dropped,
    // and capture must keep going rather than wait for the worker.
    let source = FakeSource::from_samples("sine", sine(440.0, 0.2), Pacing::Realtime);
    let rec = TrackRecorder::start_with_capacity(Box::new(source), &path, 100).unwrap();
    wait_until_finished(&rec, Duration::from_secs(5));
    let stats = rec.stop().unwrap();

    assert!(stats.overrun_samples > 0);
    assert_eq!(
        stats.samples + stats.overrun_samples,
        (0.2 * SAMPLE_RATE as f32) as u64
    );
}

#[test]
fn stopping_early_keeps_what_was_captured() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mic.opus");
    let source = FakeSource::from_samples("sine", sine(440.0, 10.0), Pacing::Realtime);

    let rec = TrackRecorder::start(Box::new(source), &path).unwrap();
    thread::sleep(Duration::from_millis(300));
    let stats = rec.stop().unwrap();

    assert!(stats.samples > 0 && stats.duration_ms < 2000, "{stats:?}");
    let (decoded, truncated) = decode(&path);
    assert!(!truncated);
    assert_eq!(decoded.len() as u64, stats.samples);
}

#[test]
fn dropping_recorder_still_finalizes_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mic.opus");
    let source = FakeSource::from_samples("sine", sine(440.0, 1.0), Pacing::Lossless);
    {
        let rec = TrackRecorder::start(Box::new(source), &path).unwrap();
        wait_until_finished(&rec, Duration::from_secs(5));
    }
    let (decoded, truncated) = decode(&path);
    assert!(!truncated);
    assert_eq!(decoded.len(), SAMPLE_RATE as usize);
}

#[test]
fn wav_fixture_is_downmixed_and_rate_checked() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("stereo.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for _ in 0..4800 {
        w.write_sample(i16::MAX / 2).unwrap();
        w.write_sample(0_i16).unwrap();
    }
    w.finalize().unwrap();

    let src = FakeSource::from_wav(&wav, Pacing::Lossless).unwrap();
    assert_eq!(src.len_samples(), 4800);

    let wrong = dir.path().join("44k.wav");
    let mut w = hound::WavWriter::create(
        &wrong,
        hound::WavSpec {
            sample_rate: 44_100,
            ..spec
        },
    )
    .unwrap();
    w.write_sample(0_i16).unwrap();
    w.write_sample(0_i16).unwrap();
    w.finalize().unwrap();
    assert!(matches!(
        FakeSource::from_wav(&wrong, Pacing::Lossless),
        Err(AudioError::UnsupportedFormat(_))
    ));
}
