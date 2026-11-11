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
//! Tracks are independent. If one fails to start or dies mid-recording, the
//! other keeps recording and the failure is reported, never hidden.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::{info, warn};

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
    /// False if the encoder worker stopped early.
    pub healthy: bool,
    /// Fault reported by the capture source.
    pub source_error: Option<String>,
}

/// Live view of a session.
#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub elapsed_ms: u64,
    pub tracks: Vec<TrackStatus>,
    /// Tracks that never started, with the reason.
    pub failed_to_start: Vec<(Track, String)>,
}

impl SessionStatus {
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
    pub duration_ms: u64,
    pub tracks: Vec<TrackSummary>,
    pub failed_to_start: Vec<(Track, String)>,
}

impl SessionSummary {
    /// True if every requested track started and finalized cleanly.
    pub fn is_clean(&self) -> bool {
        self.failed_to_start.is_empty() && self.tracks.iter().all(|t| t.outcome.is_ok())
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
        })
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
                    drift_ms: first.map(|f| {
                        let wall_ms = now.saturating_duration_since(f).as_secs_f64() * 1000.0;
                        let audio_ms =
                            stats.delivered_samples() as f64 * 1000.0 / f64::from(SAMPLE_RATE);
                        wall_ms - audio_ms
                    }),
                    drift_correction_ms: t.recorder.drift_correction_ms(),
                    healthy: t.recorder.is_healthy(),
                    source_error: t.recorder.source_error(),
                }
            })
            .collect();
        SessionStatus {
            elapsed_ms: duration_ms(self.started_at, now),
            tracks,
            failed_to_start: self.failed_to_start.clone(),
        }
    }

    /// Stops every track, drains and finalizes its file. Each track's outcome
    /// is reported separately; one failing does not affect the others.
    pub fn stop(self) -> SessionSummary {
        let duration_ms = duration_ms(self.started_at, Instant::now());
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
            tracks,
            failed_to_start: self.failed_to_start,
        }
    }
}

fn duration_ms(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}

/// `to - from` in ms, negative if `to` is earlier.
fn signed_ms(from: Instant, to: Instant) -> i64 {
    match to.checked_duration_since(from) {
        Some(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        None => -i64::try_from(from.duration_since(to).as_millis()).unwrap_or(i64::MAX),
    }
}
