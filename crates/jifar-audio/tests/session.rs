#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;
use std::thread;
use std::time::{Duration, Instant};

use common::{chirp, sine};
use jifar_audio::{
    AudioError, CaptureSource, FakeSource, OggOpusReader, Pacing, RecordingSession, RtSink,
    SAMPLE_RATE, Track,
};

fn decode(path: &std::path::Path) -> Vec<f32> {
    OggOpusReader::new(File::open(path).unwrap())
        .unwrap()
        .read_to_end()
        .unwrap()
}

fn fake(samples: Vec<f32>) -> Box<dyn CaptureSource> {
    Box::new(FakeSource::from_samples("fake", samples, Pacing::Realtime))
}

/// Waits until every track has written at least `samples`.
fn wait_for_samples(session: &RecordingSession, samples: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while session
        .status()
        .tracks
        .iter()
        .any(|t| t.samples_written < samples)
    {
        assert!(Instant::now() < deadline, "tracks did not fill in time");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A source that can't start, like a missing device.
struct BrokenSource;

impl CaptureSource for BrokenSource {
    fn name(&self) -> &str {
        "broken"
    }
    fn start(&mut self, _sink: RtSink) -> jifar_audio::Result<()> {
        Err(AudioError::Source("no such device".into()))
    }
    fn stop(&mut self) -> jifar_audio::Result<()> {
        Ok(())
    }
}

#[test]
fn records_both_tracks_into_meeting_dir() {
    let dir = tempfile::tempdir().unwrap();
    let meeting = dir.path().join("meeting");
    let mic = chirp(0.5);
    let system = sine(440.0, 0.5);

    let session = RecordingSession::start(
        &meeting,
        vec![
            (Track::Mic, fake(mic.clone())),
            (Track::System, fake(system.clone())),
        ],
    )
    .unwrap();
    wait_for_samples(&session, mic.len() as u64);

    let status = session.status();
    for t in &status.tracks {
        let offset = t.start_offset_ms.expect("first sample time");
        assert!(
            (-20..200).contains(&offset),
            "{:?} offset {offset} ms",
            t.track
        );
        assert!(t.healthy);
        assert_eq!(t.overrun_samples, 0);
    }
    // Fake sources run on the same clock as the session, so drift is tiny.
    let drift = status.inter_track_drift_ms().unwrap();
    assert!(drift.abs() < 50.0, "drift {drift} ms");

    let summary = session.stop();
    assert!(summary.is_clean());
    assert_eq!(summary.tracks.len(), 2);
    for t in &summary.tracks {
        let stats = t.outcome.as_ref().unwrap();
        assert_eq!(stats.overrun_samples, 0);
        assert_eq!(t.path, meeting.join(t.track.file_name()));
        assert_eq!(decode(&t.path).len() as u64, stats.samples);
    }
    assert_eq!(decode(&meeting.join("mic.opus")).len(), mic.len());
    assert_eq!(decode(&meeting.join("system.opus")).len(), system.len());
}

#[test]
fn one_track_failing_to_start_leaves_the_other_recording() {
    let dir = tempfile::tempdir().unwrap();
    let mic = sine(300.0, 0.3);
    let session = RecordingSession::start(
        dir.path(),
        vec![
            (Track::Mic, fake(mic.clone())),
            (Track::System, Box::new(BrokenSource)),
        ],
    )
    .unwrap();

    let status = session.status();
    assert_eq!(status.tracks.len(), 1);
    assert_eq!(status.failed_to_start.len(), 1);
    assert_eq!(status.failed_to_start[0].0, Track::System);
    assert!(status.failed_to_start[0].1.contains("no such device"));
    assert!(status.inter_track_drift_ms().is_none());

    wait_for_samples(&session, mic.len() as u64);
    let summary = session.stop();
    assert!(!summary.is_clean());
    assert_eq!(decode(&dir.path().join("mic.opus")).len(), mic.len());
    // A track that never started leaves no half-written file behind.
    assert!(!dir.path().join("system.opus").exists());
}

#[test]
fn fails_when_no_track_starts() {
    let dir = tempfile::tempdir().unwrap();
    let err = RecordingSession::start(dir.path(), vec![(Track::Mic, Box::new(BrokenSource))])
        .err()
        .unwrap();
    assert!(matches!(err, AudioError::Source(_)), "{err}");
}

#[test]
fn rejects_empty_and_duplicate_tracks() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        RecordingSession::start(dir.path(), vec![]),
        Err(AudioError::InvalidSession(_))
    ));
    let dup = vec![
        (Track::Mic, fake(vec![0.0; 10])),
        (Track::Mic, fake(vec![0.0; 10])),
    ];
    assert!(matches!(
        RecordingSession::start(dir.path(), dup),
        Err(AudioError::InvalidSession(_))
    ));
}

#[test]
fn drift_is_reported_against_the_session_clock() {
    let dir = tempfile::tempdir().unwrap();
    // 1 s of audio delivered in real time: drift should stay near zero.
    let session = RecordingSession::start(
        dir.path(),
        vec![(Track::Mic, fake(vec![0.1; SAMPLE_RATE as usize]))],
    )
    .unwrap();
    wait_for_samples(&session, u64::from(SAMPLE_RATE) / 2);
    let drift = session
        .status()
        .track(Track::Mic)
        .unwrap()
        .drift_ms
        .unwrap();
    assert!(drift.abs() < 50.0, "drift {drift} ms");
    assert!(session.stop().is_clean());
}
