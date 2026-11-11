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

#[test]
fn device_stall_is_filled_so_tracks_stay_aligned() {
    let dir = tempfile::tempdir().unwrap();
    let meeting = dir.path().join("meeting");
    // Mic hiccups for 600 ms after 2.5 s; the system track keeps time. The
    // stall is detected one 2 s envelope window after it ends (~5.1 s).
    let mic = FakeSource::from_samples(
        "mic",
        sine(300.0, 7.0),
        Pacing::Stalling {
            after_ms: 2_500,
            for_ms: 600,
        },
    );
    let system = FakeSource::from_samples("system", sine(500.0, 9.0), Pacing::Realtime);
    let session = RecordingSession::start(
        &meeting,
        vec![
            (Track::Mic, Box::new(mic) as Box<dyn CaptureSource>),
            (Track::System, Box::new(system)),
        ],
    )
    .unwrap();
    // Compare while both tracks are still live.
    thread::sleep(Duration::from_millis(6_000));
    let st = session.status();
    let residual = st.inter_track_residual_ms().unwrap();
    let written = |t| st.track(t).unwrap().samples_written as i64;
    let gap_ms = (written(Track::Mic) - written(Track::System)) * 1000 / i64::from(SAMPLE_RATE);
    // The mic needs 7.6 s of wall time to deliver its 7 s.
    thread::sleep(Duration::from_millis(1_800));
    let summary = session.stop();

    let mic = summary
        .tracks
        .iter()
        .find(|t| t.track == Track::Mic)
        .unwrap();
    let stats = mic.outcome.as_ref().unwrap();
    assert!(
        (500..=700).contains(&stats.silence_inserted_ms),
        "stall silence {} ms",
        stats.silence_inserted_ms
    );
    // The mic file covers the wall-clock span, stall included.
    let decoded = decode(&mic.path);
    let mic_ms = decoded.len() as i64 * 1000 / i64::from(SAMPLE_RATE);
    assert!(mic_ms >= 7_500, "mic file only {mic_ms} ms");
    // Same amount of audio on both tracks for the same wall time.
    assert!(gap_ms.abs() <= 20, "files {gap_ms:+} ms apart");
    // Raw drift includes delivery jitter, so this bound is looser.
    assert!(residual.abs() < 50.0, "tracks {residual:+.1} ms apart");
}

#[test]
fn pause_cuts_paused_time_out_of_both_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let meeting = dir.path().join("meeting");
    // Devices keep delivering while paused, like real ones.
    let mut session = RecordingSession::start(
        &meeting,
        vec![
            (Track::Mic, fake(sine(300.0, 10.0))),
            (Track::System, fake(sine(500.0, 10.0))),
        ],
    )
    .unwrap();
    thread::sleep(Duration::from_millis(1_500));
    session.pause();
    session.pause(); // idempotent
    assert!(session.status().paused);
    let frozen = session.status().elapsed_ms;
    let written: Vec<u64> = session
        .status()
        .tracks
        .iter()
        .map(|t| t.samples_written)
        .collect();
    thread::sleep(Duration::from_millis(2_500));
    let st = session.status();
    assert!(st.elapsed_ms - frozen <= 5, "clock ran while paused");
    for (t, before) in st.tracks.iter().zip(&written) {
        // Only the ring backlog (a few ms) may land after the pause.
        assert!(
            t.samples_written - before < 4_800,
            "{:?} kept recording",
            t.track
        );
    }
    session.resume();
    session.resume(); // idempotent
    // Run past one drift window so a mis-measured pause would show up.
    thread::sleep(Duration::from_millis(3_000));
    let st = session.status();
    let written = |t| st.track(t).unwrap().samples_written as i64;
    let gap_ms = (written(Track::Mic) - written(Track::System)) * 1000 / i64::from(SAMPLE_RATE);
    let summary = session.stop();

    assert_eq!(summary.pauses.len(), 1);
    let pause = summary.pauses[0];
    assert!((1_450..1_650).contains(&pause.at_ms), "{pause:?}");
    assert!((2_450..2_650).contains(&pause.len_ms), "{pause:?}");
    // 1.5 s + 3 s recorded; the 2.5 s pause is not in the timeline.
    assert!(
        (4_400..4_700).contains(&summary.duration_ms),
        "{} ms",
        summary.duration_ms
    );
    assert!(gap_ms.abs() <= 20, "tracks {gap_ms:+} ms apart");
    for t in &summary.tracks {
        let stats = t.outcome.as_ref().unwrap();
        // The pause must not look like a device stall.
        assert_eq!(stats.silence_inserted_ms, 0, "{:?}", t.track);
        assert!(stats.drift_correction_ms.abs() <= 20, "{:?}", t.track);
        let ms = decode(&t.path).len() as u64 * 1000 / u64::from(SAMPLE_RATE);
        assert!((4_300..4_700).contains(&ms), "{:?}: {ms} ms", t.track);
    }
}

#[test]
fn levels_and_silence_are_reported_per_track() {
    let dir = tempfile::tempdir().unwrap();
    // Mic: 0.5-amplitude tone (-9 dBFS RMS). System: nothing playing.
    let session = RecordingSession::start(
        &dir.path().join("meeting"),
        vec![
            (Track::Mic, fake(sine(300.0, 5.0))),
            (Track::System, fake(vec![0.0; SAMPLE_RATE as usize * 5])),
        ],
    )
    .unwrap();
    thread::sleep(Duration::from_millis(1_500));
    let st = session.status();
    session.stop();

    let mic = st.track(Track::Mic).unwrap();
    let level = mic.level.unwrap();
    assert!((level.rms_dbfs + 9.0).abs() < 1.0, "{level:?}");
    assert!((level.peak_dbfs + 6.0).abs() < 1.0, "{level:?}");
    assert_eq!(mic.silent_ms, 0);

    let system = st.track(Track::System).unwrap();
    assert_eq!(system.level.unwrap().rms_dbfs, jifar_audio::MIN_DBFS);
    assert!(system.silent_ms >= 1_000, "{} ms", system.silent_ms);
    assert!(!system.silence_warning(), "only after 30 s");
}

#[test]
fn a_device_that_stops_delivering_reads_as_silent() {
    let dir = tempfile::tempdir().unwrap();
    // Delivers 0.5 s of tone, then nothing, like a dead stream. Drift
    // correction fills the gap with silence, so the meter sees it.
    let session = RecordingSession::start(
        &dir.path().join("meeting"),
        vec![(Track::Mic, fake(sine(300.0, 0.5)))],
    )
    .unwrap();
    thread::sleep(Duration::from_millis(4_000));
    let mic = session.status().track(Track::Mic).cloned().unwrap();
    session.stop();
    assert!(
        mic.silent_ms >= 1_000,
        "dead mic silent for only {} ms",
        mic.silent_ms
    );
}
