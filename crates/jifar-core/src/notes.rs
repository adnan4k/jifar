//! Meeting notes (FR-AI-01..05): built from the saved transcript by a
//! [`NotesProvider`], saved as a new version each time (never overwriting
//! an earlier one, so a user's own version survives regeneration).
//!
//! Split in three steps so callers can keep database work off async
//! threads: [`NotesService::prepare`] and [`NotesService::save`] are blocking
//! (database), [`NotesService::generate`] is the network call in between.

use std::sync::Arc;

use jifar_providers::{
    LangCode, MeetingNotes, NotesInput, NotesProvider, NotesSegment, ProviderError,
};
use jifar_store::StoreError;

use crate::library::{Library, LibraryError};

#[derive(Debug, thiserror::Error)]
pub enum NotesError {
    #[error("meeting {0} not found")]
    NotFound(String),
    #[error("this meeting has no transcript yet")]
    NoTranscript,
    #[error("unsupported output language {0:?}")]
    BadLanguage(String),
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Library(#[from] LibraryError),
}

/// A saved notes version.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedNotes {
    pub version: i64,
    pub language: String,
    pub provider: Option<String>,
    pub notes: MeetingNotes,
}

#[derive(Clone)]
pub struct NotesService {
    provider: Arc<dyn NotesProvider>,
    library: Library,
}

impl NotesService {
    pub fn new(provider: Arc<dyn NotesProvider>, library: Library) -> Self {
        Self { provider, library }
    }

    /// Blocking: the transcript as notes input. `output_language` defaults
    /// to the meeting's language (FR-AI-03).
    pub fn prepare(
        &self,
        meeting_id: &str,
        output_language: Option<&str>,
    ) -> Result<NotesInput, NotesError> {
        let d = self
            .library
            .get(meeting_id)?
            .ok_or_else(|| NotesError::NotFound(meeting_id.to_owned()))?;
        if d.segments.is_empty() {
            return Err(NotesError::NoTranscript);
        }
        let meeting_language = LangCode::new(&d.meeting.language)
            .ok_or_else(|| NotesError::BadLanguage(d.meeting.language.clone()))?;
        let output_language = match output_language {
            Some(l) => LangCode::new(l).ok_or_else(|| NotesError::BadLanguage(l.to_owned()))?,
            None => meeting_language.clone(),
        };
        Ok(NotesInput {
            meeting_language,
            output_language,
            segments: d
                .segments
                .into_iter()
                .map(|s| NotesSegment {
                    speaker: if s.track == "system" { "Others" } else { "Me" }.into(),
                    id: s.id,
                    start_ms: s.start_ms,
                    text: s.text,
                })
                .collect(),
        })
    }

    /// Network: asks the provider.
    pub async fn generate(&self, input: &NotesInput) -> Result<MeetingNotes, NotesError> {
        Ok(self.provider.generate(input).await?)
    }

    /// Blocking: saves the notes as a new AI version.
    pub fn save(
        &self,
        meeting_id: &str,
        input: &NotesInput,
        notes: MeetingNotes,
    ) -> Result<SavedNotes, NotesError> {
        let body = serde_json::to_string(&notes)
            .map_err(|e| NotesError::Provider(ProviderError::permanent(e.to_string())))?;
        let language = input.output_language.as_str().to_owned();
        let provider = self.provider.id().to_owned();
        let version =
            self.library
                .store()
                .add_notes(meeting_id, "ai", Some(&provider), &language, &body)?;
        Ok(SavedNotes {
            version,
            language,
            provider: Some(provider),
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use jifar_store::{Meeting, SegmentRow, Status, Store};

    use super::*;
    use crate::paths::AppPaths;

    struct EchoNotes;

    #[async_trait]
    impl NotesProvider for EchoNotes {
        fn id(&self) -> &'static str {
            "echo"
        }
        async fn generate(&self, input: &NotesInput) -> Result<MeetingNotes, ProviderError> {
            Ok(MeetingNotes {
                summary: format!(
                    "{} lines in {}; first by {}",
                    input.segments.len(),
                    input.output_language,
                    input.segments[0].speaker
                ),
                key_points: vec![],
                decisions: vec![],
                action_items: vec![],
            })
        }
    }

    fn service(with_segments: bool) -> (NotesService, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        store
            .create_meeting(Meeting {
                id: "m".into(),
                title: "t".into(),
                language: "om".into(),
                status: Status::Ready,
                started_at: 0,
                duration_ms: 1,
            })
            .unwrap();
        if with_segments {
            store
                .add_segments(
                    "m",
                    vec![SegmentRow {
                        id: "m-system-0000-0".into(),
                        track: "system".into(),
                        start_ms: 0,
                        end_ms: 1,
                        text: "Akkam".into(),
                        edited: false,
                    }],
                )
                .unwrap();
        }
        let library = Library::new(
            store,
            AppPaths {
                data_dir: tmp.path().join("d"),
                config_dir: tmp.path().join("c"),
            },
        );
        (NotesService::new(Arc::new(EchoNotes), library), tmp)
    }

    #[tokio::test]
    async fn generates_and_saves_versions() {
        let (svc, _tmp) = service(true);
        let input = svc.prepare("m", None).unwrap();
        let notes = svc.generate(&input).await.unwrap();
        assert_eq!(notes.summary, "1 lines in om; first by Others");
        let saved = svc.save("m", &input, notes).unwrap();
        assert_eq!(saved.version, 1);

        let en = svc.prepare("m", Some("en")).unwrap();
        let notes = svc.generate(&en).await.unwrap();
        assert_eq!(svc.save("m", &en, notes).unwrap().version, 2);
        let latest = svc.library.get("m").unwrap().unwrap().notes.unwrap();
        assert_eq!((latest.version, latest.language.as_str()), (2, "en"));
    }

    #[test]
    fn refuses_without_a_transcript_or_meeting() {
        let (svc, _tmp) = service(false);
        assert!(matches!(
            svc.prepare("m", None),
            Err(NotesError::NoTranscript)
        ));
        assert!(matches!(
            svc.prepare("nope", None),
            Err(NotesError::NotFound(_))
        ));
        let (svc, _tmp) = service(true);
        assert!(matches!(
            svc.prepare("m", Some("oromo")),
            Err(NotesError::BadLanguage(_))
        ));
    }
}
