//! Post-stop transcription (SPEC D1, §6.3, §6.4): chunk each track, send the
//! chunks to the ASR provider, report segments as they arrive.
//!
//! This is the in-memory precursor of `jifar-queue`: same flow and events,
//! but state lives only while the app runs. The persistent queue (restart
//! recovery, offline → online) replaces the job table here in M3; the event
//! stream stays.
//!
//! Each chunk's request ID is derived from the meeting and chunk
//! (`<meeting>-<track>-<index>`), so a retry is an idempotent replay at the
//! provider: chunks already transcribed are never billed twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub use jifar_audio::Track;
use jifar_audio::{ChunkerConfig, SileroVad, Vad, chunk_track};
use jifar_providers::{ActionKind, AsrProvider, AudioChunk, LangCode, ProviderError, RawSegment};
use tracing::{info, warn};

use crate::recording::{MeetingInfo, TrackAudio};

/// Attempts per chunk for `Retryable` errors before it counts as failed.
const MAX_ATTEMPTS: u32 = 4;
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
/// Upper bound on a provider's `Retry-After` while the user is waiting.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEvent {
    /// Chunking finished, or a chunk finished.
    Progress {
        meeting_id: String,
        done: usize,
        failed: usize,
        total: usize,
    },
    /// New segments, on the recording clock.
    Segments {
        meeting_id: String,
        track: Track,
        chunk_index: u32,
        segments: Vec<RawSegment>,
    },
    /// Nothing can be sent until the user acts (key or balance). Remaining
    /// chunks wait for [`TranscriptionService::retry`].
    Blocked {
        meeting_id: String,
        kind: ActionKind,
        message: String,
    },
    /// Every chunk is done or failed.
    Finished {
        meeting_id: String,
        failed: usize,
        total: usize,
    },
    /// The meeting couldn't be prepared (e.g. speech detection unavailable).
    Error { meeting_id: String, message: String },
}

pub type EventSink = Arc<dyn Fn(TranscriptEvent) + Send + Sync>;
pub type VadFactory = Arc<dyn Fn() -> Result<Box<dyn Vad>, String> + Send + Sync>;

/// Silero VAD, as in the app.
pub fn silero_vad() -> VadFactory {
    Arc::new(|| {
        SileroVad::new()
            .map(|v| Box::new(v) as Box<dyn Vad>)
            .map_err(|e| e.to_string())
    })
}

#[derive(Debug, Clone, PartialEq)]
enum ChunkState {
    Pending,
    Done,
    Failed(String),
}

struct ChunkJob {
    track: Track,
    index: u32,
    chunk: AudioChunk,
    state: ChunkState,
}

struct MeetingJob {
    language: LangCode,
    chunks: Vec<ChunkJob>,
    running: bool,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RetryError {
    #[error("this meeting has no transcription to retry")]
    Unknown,
    #[error("transcription is already running")]
    Running,
}

#[derive(Clone)]
pub struct TranscriptionService {
    provider: Arc<dyn AsrProvider>,
    vad: VadFactory,
    sink: EventSink,
    jobs: Arc<Mutex<HashMap<String, MeetingJob>>>,
}

impl TranscriptionService {
    pub fn new(provider: Arc<dyn AsrProvider>, vad: VadFactory, sink: EventSink) -> Self {
        Self {
            provider,
            vad,
            sink,
            jobs: Arc::default(),
        }
    }

    /// Chunks and transcribes a stopped meeting in the background.
    pub fn start(&self, meeting: &MeetingInfo, tracks: Vec<TrackAudio>) {
        let svc = self.clone();
        let meeting = meeting.clone();
        let spawned = std::thread::Builder::new()
            .name("transcribe".into())
            .spawn(move || svc.prepare_and_run(&meeting, &tracks));
        if let Err(e) = spawned {
            warn!(error = %e, "could not start transcription thread");
        }
    }

    /// Re-runs chunks that are pending or failed (after the user fixed the
    /// key, topped up, or came back online).
    pub fn retry(&self, meeting_id: &str) -> Result<(), RetryError> {
        {
            let mut jobs = self.lock();
            let job = jobs.get_mut(meeting_id).ok_or(RetryError::Unknown)?;
            if job.running {
                return Err(RetryError::Running);
            }
            job.running = true;
            for c in &mut job.chunks {
                if matches!(c.state, ChunkState::Failed(_)) {
                    c.state = ChunkState::Pending;
                }
            }
        }
        let svc = self.clone();
        let id = meeting_id.to_owned();
        let spawned = std::thread::Builder::new()
            .name("transcribe".into())
            .spawn(move || svc.run(&id));
        if spawned.is_err()
            && let Some(job) = self.lock().get_mut(meeting_id)
        {
            job.running = false;
        }
        Ok(())
    }

    fn prepare_and_run(&self, meeting: &MeetingInfo, tracks: &[TrackAudio]) {
        let id = meeting.id.clone();
        let fail = |message: String| {
            warn!(meeting = %id, %message, "transcription could not start");
            (self.sink)(TranscriptEvent::Error {
                meeting_id: id.clone(),
                message,
            });
        };
        let Some(language) = LangCode::new(&meeting.language) else {
            return fail(format!("invalid meeting language {:?}", meeting.language));
        };
        let mut vad = match (self.vad)() {
            Ok(v) => v,
            Err(e) => return fail(format!("speech detection is unavailable: {e}")),
        };
        let chunk_dir = meeting.dir.join("chunks");
        let mut chunks = Vec::new();
        for t in tracks {
            match chunk_track(
                &t.path,
                &chunk_dir,
                t.track,
                t.start_offset_ms,
                vad.as_mut(),
                &ChunkerConfig::default(),
            ) {
                Ok(report) => chunks.extend(report.chunks.into_iter().map(|c| ChunkJob {
                    track: t.track,
                    index: c.index,
                    chunk: AudioChunk {
                        request_id: format!("{id}-{}-{:04}", t.track.as_str(), c.index),
                        path: c.path,
                        start_ms: c.start_ms,
                        end_ms: c.end_ms,
                    },
                    state: ChunkState::Pending,
                })),
                Err(e) => {
                    return fail(format!(
                        "{} track could not be chunked: {e}",
                        t.track.as_str()
                    ));
                }
            }
        }
        // Interleave by time so the transcript fills in from the start.
        chunks.sort_by_key(|c| (c.chunk.start_ms, c.track == Track::System));
        info!(meeting = %id, chunks = chunks.len(), "transcription starting");
        self.lock().insert(
            id.clone(),
            MeetingJob {
                language,
                chunks,
                running: true,
            },
        );
        self.run(&id);
    }

    /// Sends pending chunks in order until done or blocked.
    fn run(&self, meeting_id: &str) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!(error = %e, "no async runtime for transcription");
                self.set_running(meeting_id, false);
                return;
            }
        };
        self.emit_progress(meeting_id);
        while let Some((pos, chunk, language)) = self.next_pending(meeting_id) {
            match rt.block_on(self.transcribe_with_retries(&chunk, &language)) {
                Ok(segments) => {
                    let (track, index) = self.finish_chunk(meeting_id, pos, ChunkState::Done);
                    (self.sink)(TranscriptEvent::Segments {
                        meeting_id: meeting_id.to_owned(),
                        track,
                        chunk_index: index,
                        segments,
                    });
                }
                Err(ProviderError::ActionRequired { kind, message }) => {
                    self.set_running(meeting_id, false);
                    (self.sink)(TranscriptEvent::Blocked {
                        meeting_id: meeting_id.to_owned(),
                        kind,
                        message,
                    });
                    return;
                }
                Err(e) => {
                    warn!(meeting = %meeting_id, request = %chunk.request_id, error = %e, "chunk failed");
                    self.finish_chunk(meeting_id, pos, ChunkState::Failed(e.to_string()));
                }
            }
            self.emit_progress(meeting_id);
        }
        self.set_running(meeting_id, false);
        let (_, failed, total) = self.counts(meeting_id);
        info!(meeting = %meeting_id, failed, total, "transcription finished");
        (self.sink)(TranscriptEvent::Finished {
            meeting_id: meeting_id.to_owned(),
            failed,
            total,
        });
    }

    async fn transcribe_with_retries(
        &self,
        chunk: &AudioChunk,
        language: &LangCode,
    ) -> Result<Vec<RawSegment>, ProviderError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.provider.transcribe(chunk, language).await {
                Err(ProviderError::Retryable {
                    message,
                    retry_after,
                }) if attempt < MAX_ATTEMPTS => {
                    let wait = retry_after
                        .unwrap_or(BACKOFF[(attempt as usize - 1).min(BACKOFF.len() - 1)])
                        .min(MAX_RETRY_AFTER);
                    warn!(request = %chunk.request_id, attempt, %message, ?wait, "retrying chunk");
                    tokio::time::sleep(wait).await;
                }
                other => return other,
            }
        }
    }

    fn next_pending(&self, meeting_id: &str) -> Option<(usize, AudioChunk, LangCode)> {
        let jobs = self.lock();
        let job = jobs.get(meeting_id)?;
        job.chunks
            .iter()
            .position(|c| c.state == ChunkState::Pending)
            .map(|pos| (pos, job.chunks[pos].chunk.clone(), job.language.clone()))
    }

    fn finish_chunk(&self, meeting_id: &str, pos: usize, state: ChunkState) -> (Track, u32) {
        let mut jobs = self.lock();
        match jobs.get_mut(meeting_id).and_then(|j| j.chunks.get_mut(pos)) {
            Some(c) => {
                c.state = state;
                (c.track, c.index)
            }
            None => (Track::Mic, 0),
        }
    }

    fn counts(&self, meeting_id: &str) -> (usize, usize, usize) {
        let jobs = self.lock();
        let Some(job) = jobs.get(meeting_id) else {
            return (0, 0, 0);
        };
        let done = job
            .chunks
            .iter()
            .filter(|c| c.state == ChunkState::Done)
            .count();
        let failed = job
            .chunks
            .iter()
            .filter(|c| matches!(c.state, ChunkState::Failed(_)))
            .count();
        (done, failed, job.chunks.len())
    }

    fn emit_progress(&self, meeting_id: &str) {
        let (done, failed, total) = self.counts(meeting_id);
        (self.sink)(TranscriptEvent::Progress {
            meeting_id: meeting_id.to_owned(),
            done,
            failed,
            total,
        });
    }

    fn set_running(&self, meeting_id: &str, running: bool) {
        if let Some(job) = self.lock().get_mut(meeting_id) {
            job.running = running;
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, MeetingJob>> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use jifar_audio::{EnergyVad, OggOpusWriter, SAMPLE_RATE};

    use super::*;
    use crate::recording::MeetingStatus;

    /// Fails with `fail_with` for the first `fail_first` calls, then echoes
    /// the chunk's time span as a segment.
    struct FakeAsr {
        calls: AtomicUsize,
        fail_first: usize,
        fail_with: ProviderError,
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AsrProvider for FakeAsr {
        fn id(&self) -> &'static str {
            "fake"
        }
        async fn transcribe(
            &self,
            chunk: &AudioChunk,
            language: &LangCode,
        ) -> Result<Vec<RawSegment>, ProviderError> {
            assert_eq!(language.as_str(), "om");
            self.seen.lock().unwrap().push(chunk.request_id.clone());
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.fail_first {
                return Err(self.fail_with.clone());
            }
            Ok(vec![RawSegment {
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                text: "Akkam bultan".into(),
                confidence: None,
            }])
        }
    }

    fn talk_then_quiet(secs_talk: f32, secs_quiet: f32) -> Vec<f32> {
        let mut v: Vec<f32> = (0..(secs_talk * SAMPLE_RATE as f32) as usize)
            .map(|i| 0.4 * (i as f32 * 0.05).sin())
            .collect();
        v.extend(std::iter::repeat_n(
            0.0,
            (secs_quiet * SAMPLE_RATE as f32) as usize,
        ));
        v
    }

    fn meeting(dir: &std::path::Path) -> (MeetingInfo, Vec<TrackAudio>) {
        let mut tracks = Vec::new();
        for (track, offset) in [(Track::Mic, 0), (Track::System, 40)] {
            let path = dir.join(track.file_name());
            let mut w = OggOpusWriter::new(Vec::new(), 1).unwrap();
            w.write_samples(&talk_then_quiet(3.0, 1.0)).unwrap();
            std::fs::write(&path, w.finish().unwrap()).unwrap();
            tracks.push(TrackAudio {
                track,
                path,
                start_offset_ms: offset,
            });
        }
        let info = MeetingInfo {
            id: "m1".into(),
            title: "t".into(),
            language: "om".into(),
            status: MeetingStatus::Processing,
            started_at_ms: 0,
            duration_ms: 4_000,
            dir: dir.to_owned(),
        };
        (info, tracks)
    }

    fn run(
        asr: Arc<FakeAsr>,
    ) -> (
        TranscriptionService,
        Arc<Mutex<Vec<TranscriptEvent>>>,
        tempfile::TempDir,
    ) {
        let events: Arc<Mutex<Vec<TranscriptEvent>>> = Arc::default();
        let sink_events = Arc::clone(&events);
        let svc = TranscriptionService::new(
            asr,
            Arc::new(|| Ok(Box::new(EnergyVad::default()) as Box<dyn Vad>)),
            Arc::new(move |e| sink_events.lock().unwrap().push(e)),
        );
        let dir = tempfile::tempdir().unwrap();
        let (info, tracks) = meeting(dir.path());
        svc.start(&info, tracks);
        (svc, events, dir)
    }

    fn wait_for(events: &Arc<Mutex<Vec<TranscriptEvent>>>, f: impl Fn(&TranscriptEvent) -> bool) {
        for _ in 0..200 {
            if events.lock().unwrap().iter().any(&f) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("event never arrived: {:#?}", events.lock().unwrap());
    }

    fn asr(fail_first: usize, fail_with: ProviderError) -> Arc<FakeAsr> {
        Arc::new(FakeAsr {
            calls: AtomicUsize::new(0),
            fail_first,
            fail_with,
            seen: Mutex::default(),
        })
    }

    #[test]
    fn transcribes_both_tracks_in_time_order() {
        let a = asr(0, ProviderError::permanent("unused"));
        let (_svc, events, _dir) = run(Arc::clone(&a));
        wait_for(&events, |e| matches!(e, TranscriptEvent::Finished { .. }));
        let ev = events.lock().unwrap();
        let segs: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                TranscriptEvent::Segments {
                    track, segments, ..
                } => Some((*track, segments[0].start_ms)),
                _ => None,
            })
            .collect();
        assert_eq!(segs.len(), 2, "{ev:#?}");
        // Offsets applied, mic first.
        assert_eq!(segs[0].0, Track::Mic);
        assert_eq!(segs[1].0, Track::System);
        assert!(segs[1].1 >= 40);
        assert!(matches!(
            ev.last(),
            Some(TranscriptEvent::Finished {
                failed: 0,
                total: 2,
                ..
            })
        ));
        assert_eq!(
            *a.seen.lock().unwrap(),
            vec!["m1-mic-0000", "m1-system-0000"]
        );
    }

    #[test]
    fn blocked_on_missing_key_then_retry_finishes_without_resending_done_chunks() {
        let a = asr(
            1,
            ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                message: "no key".into(),
            },
        );
        let (svc, events, _dir) = run(Arc::clone(&a));
        wait_for(&events, |e| {
            matches!(
                e,
                TranscriptEvent::Blocked {
                    kind: ActionKind::Auth,
                    ..
                }
            )
        });
        assert_eq!(svc.retry("unknown"), Err(RetryError::Unknown));
        // "User adds the key."
        svc.retry("m1").unwrap();
        wait_for(&events, |e| {
            matches!(
                e,
                TranscriptEvent::Finished {
                    failed: 0,
                    total: 2,
                    ..
                }
            )
        });
        // The blocked chunk was retried; nothing was sent twice after success.
        assert_eq!(
            *a.seen.lock().unwrap(),
            vec!["m1-mic-0000", "m1-mic-0000", "m1-system-0000"]
        );
    }

    #[test]
    fn permanent_failures_finish_with_a_count() {
        let a = asr(1, ProviderError::permanent("INVALID_AUDIO"));
        let (_svc, events, _dir) = run(a);
        wait_for(&events, |e| {
            matches!(
                e,
                TranscriptEvent::Finished {
                    failed: 1,
                    total: 2,
                    ..
                }
            )
        });
    }

    #[test]
    fn missing_speech_detection_is_reported() {
        let events: Arc<Mutex<Vec<TranscriptEvent>>> = Arc::default();
        let sink_events = Arc::clone(&events);
        let svc = TranscriptionService::new(
            asr(0, ProviderError::permanent("unused")),
            Arc::new(|| Err("no onnxruntime".into())),
            Arc::new(move |e| sink_events.lock().unwrap().push(e)),
        );
        let dir = tempfile::tempdir().unwrap();
        let (info, tracks) = meeting(dir.path());
        svc.start(&info, tracks);
        wait_for(
            &events,
            |e| matches!(e, TranscriptEvent::Error { message, .. } if message.contains("onnxruntime")),
        );
    }
}
