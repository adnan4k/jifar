//! [`RecordingSession`]: the mic and system tracks of one meeting, recorded
//! side by side against one monotonic clock (FR-REC-04, SPEC §6.3).
//!
//! The session clock starts at `recording_start` (just before the first track
//! starts). Each track reports when its first sample was captured relative to
//! that clock, so the tracks can be aligned later and every timestamp stays in
//! integer ms from the start.
//!
//! Drift: per track, the wall-clock time since its first sample minus the
//! audio time the device has delivered. Each track's worker corrects its own
//! drift against this clock once it exceeds 20 ms (see the `drift` module),
//! so what remains between the tracks, [`SessionStatus::inter_track_residual_ms`],
//! stays within that bound.
//!
//! Pause/resume (FR-REC-09) cuts the paused time out of the recording: the
//! session clock stops, devices keep running but their audio is discarded,
//! and the files continue seamlessly on resume. All timestamps are recording
//! time (pauses excluded). Each track cuts at its own callback block, so a
//! pause may leave the tracks a few ms apart; drift correction runs on
//! recording time and absorbs that like any other drift.
//!
//! Tracks are independent. If one fails to start or dies mid-recording, the
//! other keeps recording and the failure is reported, never hidden.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::level::{Level, SILENCE_WARNING_MS};
use crate::recorder::{TrackRecorder, TrackStats};
use crate::source::CaptureSource;
use crate::{AudioError, Result, SAMPLE_RATE};

/// Which side of the meeting a track carries. Matches `segment.track` in the
/// schema (SPEC §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Track {
    /// The user's microphone ("Me").
    Mic,
    /// What the computer plays ("Others").
    System,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }

    /// Archive file name inside the meeting directory (SPEC §7.1).
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Mic => "mic.opus",
            Self::System => "system.opus",
        }
    }
}

/// Live view of one track.
#[derive(Debug, Clone)]
pub struct TrackStatus {
    pub track: Track,
    pub samples_written: u64,
    pub overrun_samples: u64,
    /// When the first sample was captured, in ms after the session start.
    /// `None` until the device delivers audio.
    pub start_offset_ms: Option<i64>,
    /// Wall-clock time minus delivered audio time since the first sample.
    /// Positive: the device is running slow or holding audio back. Raw, so
    /// it includes delivery jitter.
    pub drift_ms: Option<f64>,
    /// Net audio added (positive) or removed so far to hold the track to the
    /// session clock (drift correction and stall silence).
    pub drift_correction_ms: f64,
    /// Level of the last 100 ms written. `None` until audio has been written.
    pub level: Option<Level>,
    /// Current run of silence in ms of recorded audio (0 while there is
    /// sound). See [`Self::silence_warning`].
    pub silent_ms: u64,
    /// False if the encoder worker stopped early.
    pub healthy: bool,
    /// Why the worker stopped early (panic or write error). The file keeps
    /// everything recorded before it.
    pub fault: Option<String>,
    /// Fault reported by the capture source.
    pub source_error: Option<String>,
}

/// A pause, on the recording timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pause {
    /// Recording time at which the pause started (where the cut is).
    pub at_ms: u64,
    /// Wall-clock length of the pause.
    pub len_ms: u64,
}

/// Live view of a session.
#[derive(Debug, Clone)]
pub struct SessionStatus {
    /// Recording time so far (pauses excluded).
    pub elapsed_ms: u64,
    pub paused: bool,
    pub tracks: Vec<TrackStatus>,
    /// Tracks that never started, with the reason.
    pub failed_to_start: Vec<(Track, String)>,
}

impl TrackStatus {
    /// True once the track has been silent for more than 30 s (FR-REC-08):
    /// muted or wrong mic, nothing playing, or a device that stopped.
    pub fn silence_warning(&self) -> bool {
        self.silent_ms > SILENCE_WARNING_MS
    }
}

impl SessionStatus {
    /// Tracks currently over the silence limit.
    pub fn silent_tracks(&self) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| t.silence_warning())
            .map(|t| t.track)
            .collect()
    }

    pub fn track(&self, track: Track) -> Option<&TrackStatus> {
        self.tracks.iter().find(|t| t.track == track)
    }

    /// Mic drift minus system drift: how far the two tracks have moved apart
    /// since they started. `None` unless both tracks are delivering audio.
    pub fn inter_track_drift_ms(&self) -> Option<f64> {
        let mic = self.track(Track::Mic)?.drift_ms?;
        let system = self.track(Track::System)?.drift_ms?;
        Some(mic - system)
    }

    /// How far apart the tracks are in the files, after drift correction:
    /// [`Self::inter_track_drift_ms`] minus the corrections applied.
    pub fn inter_track_residual_ms(&self) -> Option<f64> {
        let mic = self.track(Track::Mic)?;
        let system = self.track(Track::System)?;
        Some(
            (mic.drift_ms? - mic.drift_correction_ms)
                - (system.drift_ms? - system.drift_correction_ms),
        )
    }
}

/// Result of one track after [`RecordingSession::stop`].
#[derive(Debug)]
pub struct TrackSummary {
    pub track: Track,
    pub path: PathBuf,
    pub start_offset_ms: Option<i64>,
    pub outcome: Result<TrackStats>,
}

/// Result of a whole session after [`RecordingSession::stop`].
#[derive(Debug)]
pub struct SessionSummary {
    pub dir: PathBuf,
    /// Recording time (pauses excluded).
    pub duration_ms: u64,
    /// Pauses in order, including one still open at stop.
    pub pauses: Vec<Pause>,
    pub tracks: Vec<TrackSummary>,
    pub failed_to_start: Vec<(Track, String)>,
}

impl SessionSummary {
    /// True if every requested track started and finalized cleanly.
    pub fn is_clean(&self) -> bool {
        self.failed_to_start.is_empty() && self.tracks.iter().all(|t| t.outcome.is_ok())
    }

    /// The meeting should be marked `needs_attention` (SPEC §6.4): a track
    /// failed to start, its worker failed, or audio was lost to overruns.
    /// Whatever was recorded is still on disk and gets processed.
    pub fn needs_attention(&self) -> bool {
        !self.problems().is_empty()
    }

    /// Human-readable reasons behind [`Self::needs_attention`].
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .failed_to_start
            .iter()
            .map(|(t, e)| format!("{} did not start: {e}", t.as_str()))
            .collect();
        for t in &self.tracks {
            match &t.outcome {
                Err(e) => out.push(format!("{}: {e}", t.track.as_str())),
                Ok(s) if s.overrun_samples > 0 => out.push(format!(
                    "{}: {} ms of audio lost to overruns",
                    t.track.as_str(),
                    s.overrun_samples * 1000 / u64::from(SAMPLE_RATE)
                )),
                Ok(_) => {}
            }
        }
        out
    }
}

struct ActiveTrack {
    track: Track,
    recorder: TrackRecorder,
}

pub struct RecordingSession {
    dir: PathBuf,
    started_at: Instant,
    tracks: Vec<ActiveTrack>,
    failed_to_start: Vec<(Track, String)>,
    pauses: Vec<Pause>,
    /// Wall time of finished pauses.
    paused_total: Duration,
    /// Start of the current pause.
    paused_since: Option<Instant>,
}

impl RecordingSession {
    /// Creates `dir` and starts one [`TrackRecorder`] per source, writing
    /// `<dir>/<track>.opus`. Succeeds if at least one track starts; tracks
    /// that fail are listed in [`SessionStatus::failed_to_start`].
    pub fn start(dir: &Path, sources: Vec<(Track, Box<dyn CaptureSource>)>) -> Result<Self> {
        if sources.is_empty() {
            return Err(AudioError::InvalidSession("no tracks".into()));
        }
        for (i, (track, _)) in sources.iter().enumerate() {
            if sources[..i].iter().any(|(t, _)| t == track) {
                return Err(AudioError::InvalidSession(format!(
                    "track {} given twice",
                    track.as_str()
                )));
            }
        }
        std::fs::create_dir_all(dir)?;

        let started_at = Instant::now();
        let mut tracks = Vec::with_capacity(sources.len());
        let mut failed_to_start = Vec::new();
        let mut first_error = None;
        for (track, source) in sources {
            match TrackRecorder::start(source, &dir.join(track.file_name())) {
                Ok(recorder) => tracks.push(ActiveTrack { track, recorder }),
                Err(e) => {
                    warn!(track = track.as_str(), error = %e, "track failed to start");
                    failed_to_start.push((track, e.to_string()));
                    first_error.get_or_insert(e);
                }
            }
        }
        if tracks.is_empty() {
            return Err(first_error
                .unwrap_or_else(|| AudioError::InvalidSession("no track started".into())));
        }
        info!(dir = %dir.display(), tracks = tracks.len(), "recording session started");
        Ok(Self {
            dir: dir.to_owned(),
            started_at,
            tracks,
            failed_to_start,
            pauses: Vec::new(),
            paused_total: Duration::ZERO,
            paused_since: None,
        })
    }

    /// Pauses every track. No-op if already paused.
    pub fn pause(&mut self) {
        if self.paused_since.is_some() {
            return;
        }
        let now = Instant::now();
        for t in &self.tracks {
            t.recorder.capture_stats().set_paused(true, now);
        }
        self.pauses.push(Pause {
            at_ms: self.recording_ms(now),
            len_ms: 0,
        });
        self.paused_since = Some(now);
        info!(at_ms = self.recording_ms(now), "recording paused");
    }

    /// Resumes every track. No-op if not paused.
    pub fn resume(&mut self) {
        let Some(since) = self.paused_since.take() else {
            return;
        };
        let now = Instant::now();
        for t in &self.tracks {
            t.recorder.capture_stats().set_paused(false, now);
        }
        let len = now.saturating_duration_since(since);
        self.paused_total += len;
        if let Some(p) = self.pauses.last_mut() {
            p.len_ms = millis(len);
        }
        info!(len_ms = millis(len), "recording resumed");
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    /// Recording time at `now`: wall time since start minus pauses.
    fn recording_ms(&self, now: Instant) -> u64 {
        let current = self
            .paused_since
            .map_or(Duration::ZERO, |s| now.saturating_duration_since(s));
        millis(
            now.saturating_duration_since(self.started_at)
                .saturating_sub(self.paused_total + current),
        )
    }

    /// The session clock's zero (`recording_start`).
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn status(&self) -> SessionStatus {
        let now = Instant::now();
        let tracks = self
            .tracks
            .iter()
            .map(|t| {
                let stats = t.recorder.capture_stats();
                let first = stats.first_sample_at();
                TrackStatus {
                    track: t.track,
                    samples_written: t.recorder.samples_written(),
                    overrun_samples: stats.overrun_samples(),
                    start_offset_ms: first.map(|f| signed_ms(self.started_at, f)),
                    drift_ms: stats.active_since_first(now).map(|active| {
                        let wall_ms = active.as_secs_f64() * 1000.0;
                        let audio_ms =
                            stats.delivered_samples() as f64 * 1000.0 / f64::from(SAMPLE_RATE);
                        wall_ms - audio_ms
                    }),
                    drift_correction_ms: t.recorder.drift_correction_ms(),
                    level: t.recorder.level(),
                    silent_ms: t.recorder.silent_ms(),
                    healthy: t.recorder.is_healthy(),
                    fault: t.recorder.fault(),
                    source_error: t.recorder.source_error(),
                }
            })
            .collect();
        SessionStatus {
            elapsed_ms: self.recording_ms(now),
            paused: self.is_paused(),
            tracks,
            failed_to_start: self.failed_to_start.clone(),
        }
    }

    /// Stops every track, drains and finalizes its file. Each track's outcome
    /// is reported separately; one failing does not affect the others.
    pub fn stop(mut self) -> SessionSummary {
        let now = Instant::now();
        let duration_ms = self.recording_ms(now);
        if let (Some(since), Some(p)) = (self.paused_since, self.pauses.last_mut()) {
            p.len_ms = millis(now.saturating_duration_since(since));
        }
        let started_at = self.started_at;
        let tracks = self
            .tracks
            .into_iter()
            .map(|t| {
                let start_offset_ms = t
                    .recorder
                    .capture_stats()
                    .first_sample_at()
                    .map(|f| signed_ms(started_at, f));
                let path = t.recorder.path().to_owned();
                let outcome = t.recorder.stop();
                if let Err(e) = &outcome {
                    warn!(track = t.track.as_str(), error = %e, "track did not finalize cleanly");
                }
                TrackSummary {
                    track: t.track,
                    path,
                    start_offset_ms,
                    outcome,
                }
            })
            .collect();
        info!(dir = %self.dir.display(), duration_ms, "recording session stopped");
        SessionSummary {
            dir: self.dir,
            duration_ms,
            pauses: self.pauses,
            tracks,
            failed_to_start: self.failed_to_start,
        }
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `to - from` in ms, negative if `to` is earlier.
fn signed_ms(from: Instant, to: Instant) -> i64 {
    match to.checked_duration_since(from) {
        Some(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        None => -i64::try_from(from.duration_since(to).as_millis()).unwrap_or(i64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::recorder::PANIC_SENTINEL_BITS;
    use crate::{FakeSource, OggOpusReader, Pacing};

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| 0.3 * (i as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn worker_panic_keeps_the_file_and_flags_the_meeting() {
        let dir = tempfile::tempdir().unwrap();
        // The mic worker panics 1 s in; the system track is fine.
        let mut mic = tone(2.0);
        mic[SAMPLE_RATE as usize] = f32::from_bits(PANIC_SENTINEL_BITS);
        let session = RecordingSession::start(
            dir.path(),
            vec![
                (
                    Track::Mic,
                    Box::new(FakeSource::from_samples("mic", mic, Pacing::Realtime))
                        as Box<dyn CaptureSource>,
                ),
                (
                    Track::System,
                    Box::new(FakeSource::from_samples(
                        "system",
                        tone(2.0),
                        Pacing::Realtime,
                    )),
                ),
            ],
        )
        .unwrap();
        thread::sleep(Duration::from_millis(1_600));
        let st = session.status();
        let mic_status = st.track(Track::Mic).unwrap();
        assert!(!mic_status.healthy);
        assert!(
            mic_status
                .fault
                .as_deref()
                .unwrap()
                .contains("injected test panic"),
            "{:?}",
            mic_status.fault
        );
        assert!(st.track(Track::System).unwrap().healthy);
        thread::sleep(Duration::from_millis(600));
        let summary = session.stop();

        assert!(summary.needs_attention());
        let problems = summary.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("mic:"), "{problems:?}");
        assert!(problems[0].contains("file finalized"), "{problems:?}");

        // The mic file is complete up to the panic and properly finalized.
        let mut reader =
            OggOpusReader::new(File::open(dir.path().join("mic.opus")).unwrap()).unwrap();
        let decoded = reader.read_to_end().unwrap();
        assert!(!reader.is_truncated(), "mic.opus should be finalized");
        let ms = decoded.len() * 1000 / SAMPLE_RATE as usize;
        assert!((900..=1_100).contains(&ms), "kept {ms} ms");

        // The other track is unaffected.
        let system = summary
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert!(system.outcome.is_ok());
    }

    #[test]
    fn clean_session_needs_no_attention() {
        let dir = tempfile::tempdir().unwrap();
        let session = RecordingSession::start(
            dir.path(),
            vec![(
                Track::Mic,
                Box::new(FakeSource::from_samples("mic", tone(0.3), Pacing::Lossless))
                    as Box<dyn CaptureSource>,
            )],
        )
        .unwrap();
        thread::sleep(Duration::from_millis(200));
        let summary = session.stop();
        assert!(!summary.needs_attention(), "{:?}", summary.problems());
    }
}
