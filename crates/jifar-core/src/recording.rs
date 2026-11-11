//! [`RecordingService`]: starts, pauses and stops a meeting recording and
//! reports its live state (FR-REC-01, 07, 08, 09).
//!
//! One recording at a time. Audio goes to the meeting's directory
//! (`meetings/<id>/mic.opus`, `system.opus`, SPEC §7.1). Nothing here touches
//! the network: transcription is queued after stop, never during (SPEC D1).
//!
//! Until `jifar-store` and the queue land (M3), a stopped meeting is reported
//! as `processing` (waiting for transcription) or `needs_attention`, but not
//! yet persisted or chunked.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use jifar_audio::{
    AudioDevice, AudioError, Backend, CaptureSource, CaptureTarget, MixInput, RecordingSession,
    SessionStatus, Track, render_mixed, resolve_device,
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::paths::AppPaths;
use jifar_store::{Meeting as StoredMeeting, Status as StoredStatus, Store, TrackRow};

pub use jifar_audio::{AudioDevice as Device, DeviceKind};

/// Where capture sources come from: the real audio server, or fakes in tests.
pub trait SourceProvider: Send + Sync {
    fn list_devices(&self) -> Result<Vec<AudioDevice>, AudioError>;
    fn open(
        &self,
        target: CaptureTarget,
        device: Option<&str>,
    ) -> Result<Box<dyn CaptureSource>, AudioError>;
}

/// The machine's audio server. The backend is detected once, on first use.
#[derive(Default)]
pub struct SystemAudio {
    backend: OnceLock<Result<Backend, String>>,
}

impl SystemAudio {
    fn backend(&self) -> Result<Backend, AudioError> {
        self.backend
            .get_or_init(|| Backend::detect().map_err(|e| e.to_string()))
            .clone()
            .map_err(AudioError::Source)
    }
}

impl SourceProvider for SystemAudio {
    fn list_devices(&self) -> Result<Vec<AudioDevice>, AudioError> {
        self.backend()?.list_devices()
    }

    fn open(
        &self,
        target: CaptureTarget,
        device: Option<&str>,
    ) -> Result<Box<dyn CaptureSource>, AudioError> {
        self.backend()?.open(target, device)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingStatus {
    Recording,
    Processing,
    Ready,
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeetingInfo {
    /// UUIDv7 (time-sortable, SPEC §7.1).
    pub id: String,
    pub title: String,
    /// ISO 639-1 (`om` for Afaan Oromo).
    pub language: String,
    pub status: MeetingStatus,
    /// Wall-clock start, ms since the Unix epoch.
    pub started_at_ms: i64,
    /// Recording time (pauses excluded).
    pub duration_ms: u64,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub title: Option<String>,
    pub language: String,
    /// Input device ID; `None` for the system default.
    pub mic_device: Option<String>,
    /// Output (sink) device ID; `None` for the system default.
    pub sink_device: Option<String>,
}

#[derive(Debug)]
pub struct Started {
    pub meeting: MeetingInfo,
    /// Things the user should know, e.g. a chosen device was missing and
    /// the default is used, or one track could not start.
    pub notices: Vec<String>,
}

/// One finalized track archive, ready for chunking.
#[derive(Debug, Clone)]
pub struct TrackAudio {
    pub track: Track,
    pub path: PathBuf,
    /// Where its first sample sits on the recording clock.
    pub start_offset_ms: i64,
}

#[derive(Debug)]
pub struct Stopped {
    pub meeting: MeetingInfo,
    /// Tracks with audio on disk (input for transcription).
    pub tracks: Vec<TrackAudio>,
    /// Why the meeting needs attention; empty if the recording is clean.
    pub problems: Vec<String>,
    /// Waveform overview of `mixed.opus` (0..=1), empty if mixing failed.
    pub peaks: Vec<f32>,
}

/// Bars in the waveform overview returned on stop.
pub const WAVEFORM_BARS: usize = 400;

/// Live state of one track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackLive {
    /// RMS of the last 100 ms in dBFS; `None` before any audio.
    pub level_dbfs: Option<f32>,
    pub silent_ms: u64,
    pub silence_warning: bool,
    /// The track's worker stopped early; earlier audio is kept.
    pub fault: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveStatus {
    pub meeting_id: String,
    pub elapsed_ms: u64,
    pub paused: bool,
    pub mic: Option<TrackLive>,
    pub system: Option<TrackLive>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error("a recording is already in progress")]
    AlreadyRecording,
    #[error("no recording in progress")]
    NotRecording,
    #[error("meeting {0} is not the one being recorded")]
    WrongMeeting(String),
    #[error("audio: {0}")]
    Audio(#[from] AudioError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

struct Active {
    meeting: MeetingInfo,
    session: RecordingSession,
}

pub struct RecordingService {
    paths: AppPaths,
    audio: Box<dyn SourceProvider>,
    store: Store,
    active: Mutex<Option<Active>>,
}

impl RecordingService {
    pub fn new(paths: AppPaths, audio: Box<dyn SourceProvider>, store: Store) -> Self {
        Self {
            paths,
            audio,
            store,
            active: Mutex::new(None),
        }
    }

    /// Inputs and outputs, defaults marked (FR-REC-07).
    pub fn devices(&self) -> Result<Vec<AudioDevice>, RecordingError> {
        Ok(self.audio.list_devices()?)
    }

    pub fn is_recording(&self) -> bool {
        self.lock().is_some()
    }

    /// Starts recording both tracks. A track that fails to start is reported
    /// in `notices` while the other records; fails only if neither starts.
    pub fn start(&self, opts: StartOptions) -> Result<Started, RecordingError> {
        let mut active = self.lock();
        if active.is_some() {
            return Err(RecordingError::AlreadyRecording);
        }
        let mut notices = Vec::new();
        let wanted = opts.mic_device.is_some() || opts.sink_device.is_some();
        let devices = if wanted {
            self.audio.list_devices().unwrap_or_else(|e| {
                notices.push(format!("could not list devices ({e}); using the defaults"));
                Vec::new()
            })
        } else {
            Vec::new()
        };

        let mut sources: Vec<(Track, Box<dyn CaptureSource>)> = Vec::new();
        for (track, target, want) in [
            (
                Track::Mic,
                CaptureTarget::Microphone,
                opts.mic_device.as_deref(),
            ),
            (
                Track::System,
                CaptureTarget::SystemOutput,
                opts.sink_device.as_deref(),
            ),
        ] {
            let choice = resolve_device(&devices, target, want);
            if let Some(missing) = &choice.fell_back_from {
                let what = match DeviceKind::for_target(target) {
                    DeviceKind::Input => "microphone",
                    DeviceKind::Output => "output",
                };
                notices.push(format!(
                    "the selected {what} ({missing}) is not available; using the default"
                ));
            }
            match self.audio.open(target, choice.id.as_deref()) {
                Ok(source) => sources.push((track, source)),
                Err(e) => notices.push(format!("{} could not be opened: {e}", track.as_str())),
            }
        }
        if sources.is_empty() {
            return Err(RecordingError::Audio(AudioError::Source(
                notices.join("; "),
            )));
        }

        let id = Uuid::now_v7().to_string();
        let dir = self.paths.meeting_dir(&id);
        let session = RecordingSession::start(&dir, sources)?;
        for (track, why) in &session.status().failed_to_start {
            notices.push(format!("{} did not start: {why}", track.as_str()));
        }
        let meeting = MeetingInfo {
            id,
            title: opts
                .title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Untitled meeting".to_owned()),
            language: opts.language,
            status: MeetingStatus::Recording,
            started_at_ms: now_epoch_ms(),
            duration_ms: 0,
            dir,
        };
        // The database is bookkeeping: if it fails, keep recording (the
        // files are what matters) and say so.
        if let Err(e) = self.store.create_meeting(StoredMeeting {
            id: meeting.id.clone(),
            title: meeting.title.clone(),
            language: meeting.language.clone(),
            status: StoredStatus::Recording,
            started_at: meeting.started_at_ms,
            duration_ms: 0,
        }) {
            warn!(meeting = %meeting.id, error = %e, "could not save the meeting");
            notices.push(format!("the meeting could not be saved to history: {e}"));
        }
        info!(meeting = %meeting.id, notices = notices.len(), "recording started");
        *active = Some(Active {
            meeting: meeting.clone(),
            session,
        });
        Ok(Started { meeting, notices })
    }

    pub fn pause(&self, meeting_id: &str) -> Result<(), RecordingError> {
        self.with_active(meeting_id, |a| a.session.pause())
    }

    pub fn resume(&self, meeting_id: &str) -> Result<(), RecordingError> {
        self.with_active(meeting_id, |a| a.session.resume())
    }

    /// Stops and finalizes both tracks.
    pub fn stop(&self, meeting_id: &str) -> Result<Stopped, RecordingError> {
        let mut guard = self.lock();
        match guard.as_ref() {
            None => return Err(RecordingError::NotRecording),
            Some(a) if a.meeting.id != meeting_id => {
                return Err(RecordingError::WrongMeeting(meeting_id.to_owned()));
            }
            Some(_) => {}
        }
        let Some(Active {
            mut meeting,
            session,
        }) = guard.take()
        else {
            return Err(RecordingError::NotRecording);
        };
        drop(guard);
        let summary = session.stop();
        let problems = summary.problems();
        meeting.duration_ms = summary.duration_ms;

        // mixed.opus for playback and export (SPEC §6.3). It is derived from
        // the finalized tracks, so failing here loses nothing.
        // TODO(M3): move this into the post-stop pipeline with chunking, off
        // the stop call (a 60 min meeting takes several seconds).
        let tracks: Vec<TrackAudio> = summary
            .tracks
            .iter()
            .filter(|t| t.outcome.is_ok() || t.path.is_file())
            .map(|t| TrackAudio {
                track: t.track,
                path: t.path.clone(),
                start_offset_ms: t.start_offset_ms.unwrap_or(0),
            })
            .collect();
        let inputs: Vec<MixInput> = tracks
            .iter()
            .map(|t| MixInput {
                path: t.path.clone(),
                start_offset_ms: t.start_offset_ms,
            })
            .collect();
        let peaks = match render_mixed(&inputs, &meeting.dir.join("mixed.opus"), WAVEFORM_BARS) {
            Ok(report) => report.peaks,
            Err(e) => {
                warn!(meeting = %meeting.id, error = %e, "could not render mixed.opus");
                Vec::new()
            }
        };
        save_waveform(&meeting.dir, &peaks);
        // Waiting for transcription, unless the recording itself had trouble.
        meeting.status = if problems.is_empty() {
            MeetingStatus::Processing
        } else {
            warn!(meeting = %meeting.id, ?problems, "recording needs attention");
            MeetingStatus::NeedsAttention
        };
        let mut rows: Vec<TrackRow> = tracks
            .iter()
            .map(|t| TrackRow {
                kind: t.track.as_str().to_owned(),
                path: t.path.display().to_string(),
                duration_ms: i64::try_from(meeting.duration_ms).unwrap_or(i64::MAX),
                start_offset_ms: t.start_offset_ms,
            })
            .collect();
        let mixed = meeting.dir.join("mixed.opus");
        if mixed.is_file() {
            rows.push(TrackRow {
                kind: "mixed".into(),
                path: mixed.display().to_string(),
                duration_ms: i64::try_from(meeting.duration_ms).unwrap_or(i64::MAX),
                start_offset_ms: 0,
            });
        }
        if let Err(e) = self.store.finish_recording(
            &meeting.id,
            i64::try_from(meeting.duration_ms).unwrap_or(i64::MAX),
            to_stored(meeting.status),
            rows,
        ) {
            warn!(meeting = %meeting.id, error = %e, "could not save the finished meeting");
        }
        info!(meeting = %meeting.id, duration_ms = meeting.duration_ms, "recording stopped");
        Ok(Stopped {
            meeting,
            tracks,
            problems,
            peaks,
        })
    }

    /// Stops whatever is recording (app exit). Files are finalized.
    pub fn stop_any(&self) -> Option<Stopped> {
        let id = self.lock().as_ref().map(|a| a.meeting.id.clone())?;
        self.stop(&id).ok()
    }

    /// `None` when idle.
    pub fn live(&self) -> Option<LiveStatus> {
        let guard = self.lock();
        let active = guard.as_ref()?;
        let st: SessionStatus = active.session.status();
        let track = |t: Track| {
            st.track(t).map(|s| TrackLive {
                level_dbfs: s.level.map(|l| l.rms_dbfs),
                silent_ms: s.silent_ms,
                silence_warning: s.silence_warning(),
                fault: s.fault.clone(),
            })
        };
        Some(LiveStatus {
            meeting_id: active.meeting.id.clone(),
            elapsed_ms: st.elapsed_ms,
            paused: st.paused,
            mic: track(Track::Mic),
            system: track(Track::System),
        })
    }

    fn with_active(
        &self,
        meeting_id: &str,
        f: impl FnOnce(&mut Active),
    ) -> Result<(), RecordingError> {
        let mut guard = self.lock();
        match guard.as_mut() {
            None => Err(RecordingError::NotRecording),
            Some(a) if a.meeting.id != meeting_id => {
                Err(RecordingError::WrongMeeting(meeting_id.to_owned()))
            }
            Some(a) => {
                f(a);
                Ok(())
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Active>> {
        // A panic while holding the lock can't leave `Active` half-updated in
        // a way that matters: the session itself stays consistent.
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for RecordingService {
    fn drop(&mut self) {
        if self.stop_any().is_some() {
            info!("active recording finalized on shutdown");
        }
    }
}

pub(crate) fn to_stored(s: MeetingStatus) -> StoredStatus {
    match s {
        MeetingStatus::Recording => StoredStatus::Recording,
        MeetingStatus::Processing => StoredStatus::Processing,
        MeetingStatus::Ready => StoredStatus::Ready,
        MeetingStatus::NeedsAttention => StoredStatus::NeedsAttention,
    }
}

pub(crate) fn from_stored(s: StoredStatus) -> MeetingStatus {
    match s {
        StoredStatus::Recording => MeetingStatus::Recording,
        StoredStatus::Processing => MeetingStatus::Processing,
        StoredStatus::Ready => MeetingStatus::Ready,
        StoredStatus::NeedsAttention => MeetingStatus::NeedsAttention,
    }
}

/// `waveform.json` next to the audio: the overview drawn by the player.
/// Derived data, so a failure only costs the drawing.
fn save_waveform(dir: &std::path::Path, peaks: &[f32]) {
    let rounded: Vec<f32> = peaks
        .iter()
        .map(|p| (p * 1000.0).round() / 1000.0)
        .collect();
    if let Ok(json) = serde_json::to_vec(&rounded)
        && let Err(e) = std::fs::write(dir.join(crate::library::WAVEFORM_FILE), json)
    {
        warn!(error = %e, "could not save the waveform");
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use jifar_audio::{FakeSource, Pacing, SAMPLE_RATE};

    use super::*;

    /// Two fake devices of each kind; opening an unknown ID fails.
    struct FakeAudio;

    fn dev(id: &str, kind: DeviceKind, is_default: bool) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            description: id.into(),
            kind,
            is_default,
        }
    }

    impl SourceProvider for FakeAudio {
        fn list_devices(&self) -> Result<Vec<AudioDevice>, AudioError> {
            Ok(vec![
                dev("mic-a", DeviceKind::Input, true),
                dev("mic-b", DeviceKind::Input, false),
                dev("out-a", DeviceKind::Output, true),
            ])
        }

        fn open(
            &self,
            target: CaptureTarget,
            device: Option<&str>,
        ) -> Result<Box<dyn CaptureSource>, AudioError> {
            let tone: Vec<f32> = (0..SAMPLE_RATE as usize * 30)
                .map(|i| 0.3 * (i as f32 * 0.05).sin())
                .collect();
            let name = format!("{target:?}:{}", device.unwrap_or("default"));
            Ok(Box::new(FakeSource::from_samples(
                name,
                tone,
                Pacing::Realtime,
            )))
        }
    }

    fn service() -> (RecordingService, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_dir: tmp.path().join("data"),
            config_dir: tmp.path().join("config"),
        };
        let store = Store::open_in_memory().unwrap();
        (
            RecordingService::new(paths, Box::new(FakeAudio), store),
            tmp,
        )
    }

    fn opts() -> StartOptions {
        StartOptions {
            title: Some("Walgahii".into()),
            language: "om".into(),
            ..Default::default()
        }
    }

    #[test]
    fn full_cycle_writes_both_tracks_into_the_meeting_dir() {
        let (svc, _tmp) = service();
        let started = svc.start(opts()).unwrap();
        let id = started.meeting.id.clone();
        assert!(started.notices.is_empty(), "{:?}", started.notices);
        assert_eq!(started.meeting.status, MeetingStatus::Recording);
        assert_eq!(started.meeting.title, "Walgahii");
        assert!(Uuid::parse_str(&id).is_ok());

        thread::sleep(Duration::from_millis(400));
        let live = svc.live().unwrap();
        assert_eq!(live.meeting_id, id);
        assert!(live.mic.as_ref().unwrap().level_dbfs.is_some());

        svc.pause(&id).unwrap();
        assert!(svc.live().unwrap().paused);
        svc.resume(&id).unwrap();

        let stopped = svc.stop(&id).unwrap();
        assert!(stopped.problems.is_empty(), "{:?}", stopped.problems);
        assert_eq!(stopped.meeting.status, MeetingStatus::Processing);
        assert!(stopped.meeting.duration_ms >= 300);
        assert!(stopped.meeting.dir.join("mic.opus").is_file());
        assert!(stopped.meeting.dir.join("system.opus").is_file());
        assert!(stopped.meeting.dir.join("mixed.opus").is_file());
        assert!(!stopped.peaks.is_empty());
        assert!(svc.live().is_none());
        // Saved to history with its tracks.
        let saved = svc.store.get_meeting(&id).unwrap().unwrap();
        assert_eq!(saved.meeting.title, "Walgahii");
        assert_eq!(saved.meeting.status, jifar_store::Status::Processing);
        let kinds: Vec<&str> = saved.tracks.iter().map(|t| t.kind.as_str()).collect();
        assert_eq!(kinds, vec!["mic", "mixed", "system"]);
        assert!(stopped.meeting.dir.join("waveform.json").is_file());
    }

    #[test]
    fn one_recording_at_a_time_and_ids_are_checked() {
        let (svc, _tmp) = service();
        let id = svc.start(opts()).unwrap().meeting.id;
        assert!(matches!(
            svc.start(opts()),
            Err(RecordingError::AlreadyRecording)
        ));
        assert!(matches!(
            svc.pause("other"),
            Err(RecordingError::WrongMeeting(_))
        ));
        assert!(matches!(
            svc.stop("other"),
            Err(RecordingError::WrongMeeting(_))
        ));
        svc.stop(&id).unwrap();
        assert!(matches!(svc.stop(&id), Err(RecordingError::NotRecording)));
    }

    #[test]
    fn missing_device_falls_back_with_a_notice() {
        let (svc, _tmp) = service();
        let started = svc
            .start(StartOptions {
                mic_device: Some("unplugged-headset".into()),
                sink_device: Some("out-a".into()),
                ..opts()
            })
            .unwrap();
        assert_eq!(started.notices.len(), 1, "{:?}", started.notices);
        assert!(started.notices[0].contains("unplugged-headset"));
        svc.stop(&started.meeting.id).unwrap();
    }

    #[test]
    fn blank_title_gets_a_default() {
        let (svc, _tmp) = service();
        let started = svc
            .start(StartOptions {
                title: Some("  ".into()),
                ..opts()
            })
            .unwrap();
        assert_eq!(started.meeting.title, "Untitled meeting");
        svc.stop(&started.meeting.id).unwrap();
    }

    #[test]
    fn dropping_the_service_finalizes_the_recording() {
        let (svc, _tmp) = service();
        let dir = svc.start(opts()).unwrap().meeting.dir;
        thread::sleep(Duration::from_millis(200));
        drop(svc);
        let mut reader =
            jifar_audio::OggOpusReader::new(std::fs::File::open(dir.join("mic.opus")).unwrap())
                .unwrap();
        reader.read_to_end().unwrap();
        assert!(!reader.is_truncated(), "file must be finalized on shutdown");
    }
}
