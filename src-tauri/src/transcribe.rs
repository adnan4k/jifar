//! Wires `jifar-core`'s post-stop transcription to Tauri events.

use std::sync::Arc;

use jifar_core::recording::{MeetingStatus, Stopped};
use jifar_core::transcription::{Track, TranscriptEvent, TranscriptionService, silero_vad};
use jifar_providers::{ActionKind, AddisScribe, AsrProvider, KeyringStore};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::dto::MeetingStatusDto;
use crate::events::{
    BlockedReason, MeetingStatusChanged, SegmentDto, TrackDto, TranscriptProgress,
    TranscriptSegments,
};

/// Builds the service with Addis Scribe (key from the keyring) and Silero.
pub fn service(app: &AppHandle) -> Result<TranscriptionService, jifar_providers::ProviderError> {
    let provider: Arc<dyn AsrProvider> = Arc::new(AddisScribe::new(Box::new(KeyringStore))?);
    let app = app.clone();
    Ok(TranscriptionService::new(
        provider,
        silero_vad(),
        Arc::new(move |event| forward(&app, event)),
    ))
}

/// Starts transcription for a just-stopped meeting (SPEC D1: after stop only).
pub fn after_stop(app: &AppHandle, stopped: &Stopped) {
    if stopped.tracks.is_empty() {
        return;
    }
    if let Some(svc) = app.try_state::<TranscriptionService>() {
        svc.start(&stopped.meeting, stopped.tracks.clone());
    }
}

fn track(t: Track) -> TrackDto {
    match t {
        Track::Mic => TrackDto::Mic,
        Track::System => TrackDto::System,
    }
}

fn progress(meeting_id: String, done: usize, failed: usize, total: usize) -> TranscriptProgress {
    let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    TranscriptProgress {
        meeting_id,
        done: n(done),
        failed: n(failed),
        total: n(total),
        finished: false,
        blocked: None,
        message: None,
    }
}

fn status(app: &AppHandle, meeting_id: String, status: MeetingStatus) {
    let _ = MeetingStatusChanged {
        meeting_id,
        status: MeetingStatusDto::from(status),
    }
    .emit(app);
}

fn forward(app: &AppHandle, event: TranscriptEvent) {
    match event {
        TranscriptEvent::Progress {
            meeting_id,
            done,
            failed,
            total,
        } => {
            let _ = progress(meeting_id, done, failed, total).emit(app);
        }
        TranscriptEvent::Segments {
            meeting_id,
            track: t,
            chunk_index,
            segments,
        } => {
            let segments = segments
                .into_iter()
                .enumerate()
                .map(|(i, s)| SegmentDto {
                    id: format!("{}-{chunk_index:04}-{i}", t.as_str()),
                    track: track(t),
                    start_ms: s.start_ms as f64,
                    end_ms: s.end_ms as f64,
                    text: s.text,
                })
                .collect();
            let _ = TranscriptSegments {
                meeting_id,
                segments,
            }
            .emit(app);
        }
        TranscriptEvent::Blocked {
            meeting_id,
            kind,
            message,
        } => {
            let _ = TranscriptProgress {
                blocked: Some(match kind {
                    ActionKind::Auth => BlockedReason::Auth,
                    ActionKind::Credits => BlockedReason::Credits,
                }),
                message: Some(message),
                ..progress(meeting_id, 0, 0, 0)
            }
            .emit(app);
        }
        TranscriptEvent::Finished {
            meeting_id,
            failed,
            total,
        } => {
            let _ = TranscriptProgress {
                finished: true,
                ..progress(meeting_id.clone(), total - failed, failed, total)
            }
            .emit(app);
            status(
                app,
                meeting_id,
                if failed == 0 {
                    MeetingStatus::Ready
                } else {
                    MeetingStatus::NeedsAttention
                },
            );
        }
        TranscriptEvent::Error {
            meeting_id,
            message,
        } => {
            let _ = TranscriptProgress {
                finished: true,
                message: Some(message),
                ..progress(meeting_id.clone(), 0, 0, 0)
            }
            .emit(app);
            status(app, meeting_id, MeetingStatus::NeedsAttention);
        }
    }
}
