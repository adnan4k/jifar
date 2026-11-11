//! Meeting history (FR-HIS-01, 03, 04): list, open, rename and delete saved
//! meetings. Rows come from `jifar-store`; audio lives in `meetings/<id>/`.

use std::path::PathBuf;

use jifar_audio::Track;
use jifar_store::{SegmentRow, Store, StoreError};
use tracing::{info, warn};

use crate::paths::AppPaths;
use crate::recording::{MeetingInfo, TrackAudio, from_stored};

/// The player's waveform overview, saved on stop next to the audio.
pub const WAVEFORM_FILE: &str = "waveform.json";

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("meeting {0} not found")]
    NotFound(String),
    #[error("the title can't be empty")]
    EmptyTitle,
    #[error("deleting the meeting's files failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct MeetingDetail {
    pub meeting: MeetingInfo,
    pub segments: Vec<SegmentRow>,
    /// Waveform overview (empty if none was saved).
    pub peaks: Vec<f32>,
    /// `mixed.opus` exists, so the player can play it.
    pub has_audio: bool,
}

#[derive(Clone)]
pub struct Library {
    store: Store,
    paths: AppPaths,
}

impl Library {
    pub fn new(store: Store, paths: AppPaths) -> Self {
        Self { store, paths }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Newest first; `query` matches titles and transcript words.
    pub fn list(&self, query: Option<&str>, limit: u32) -> Result<Vec<MeetingInfo>, LibraryError> {
        Ok(self
            .store
            .list_meetings(query, limit)?
            .into_iter()
            .map(|m| self.info(m))
            .collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<MeetingDetail>, LibraryError> {
        let Some(d) = self.store.get_meeting(id)? else {
            return Ok(None);
        };
        let meeting = self.info(d.meeting);
        let peaks = std::fs::read(meeting.dir.join(WAVEFORM_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let has_audio = meeting.dir.join("mixed.opus").is_file();
        Ok(Some(MeetingDetail {
            meeting,
            segments: d.segments,
            peaks,
            has_audio,
        }))
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<(), LibraryError> {
        if title.trim().is_empty() {
            return Err(LibraryError::EmptyTitle);
        }
        self.store.rename(id, title).map_err(|e| match e {
            StoreError::NotFound(id) => LibraryError::NotFound(id),
            other => other.into(),
        })
    }

    /// Removes the meeting from history and deletes its audio and chunks
    /// (FR-HIS-04; the UI confirms first). Only `meetings/<id>/` is touched.
    pub fn delete(&self, id: &str) -> Result<(), LibraryError> {
        if !is_meeting_id(id) {
            return Err(LibraryError::NotFound(id.to_owned()));
        }
        self.store.delete(id)?;
        let dir = self.paths.meeting_dir(id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => info!(meeting = id, "meeting deleted"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!(meeting = id, error = %e, "meeting removed from history, but its files remain");
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// A saved meeting's tracks, for (re-)running its transcription.
    pub fn for_transcription(
        &self,
        id: &str,
    ) -> Result<Option<(MeetingInfo, Vec<TrackAudio>)>, LibraryError> {
        let Some(d) = self.store.get_meeting(id)? else {
            return Ok(None);
        };
        let tracks = d
            .tracks
            .iter()
            .filter_map(|t| {
                let track = match t.kind.as_str() {
                    "mic" => Track::Mic,
                    "system" => Track::System,
                    _ => return None,
                };
                let path = PathBuf::from(&t.path);
                path.is_file().then_some(TrackAudio {
                    track,
                    path,
                    start_offset_ms: t.start_offset_ms,
                })
            })
            .collect();
        Ok(Some((self.info(d.meeting), tracks)))
    }

    fn info(&self, m: jifar_store::Meeting) -> MeetingInfo {
        MeetingInfo {
            dir: self.paths.meeting_dir(&m.id),
            id: m.id,
            title: m.title,
            language: m.language,
            status: from_stored(m.status),
            started_at_ms: m.started_at,
            duration_ms: u64::try_from(m.duration_ms).unwrap_or(0),
        }
    }
}

/// UUID-shaped: hex digits and dashes only, so a bad ID can never turn
/// `meetings/<id>` into a path outside it.
fn is_meeting_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

#[cfg(test)]
mod tests {
    use jifar_store::{Meeting, Status};

    use super::*;

    const ID: &str = "01a1cba5-b95a-7536-8b7c-f11611596675";

    fn library() -> (Library, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_dir: tmp.path().join("data"),
            config_dir: tmp.path().join("config"),
        };
        let store = Store::open_in_memory().unwrap();
        store
            .create_meeting(Meeting {
                id: ID.into(),
                title: "Walgahii".into(),
                language: "om".into(),
                status: Status::Ready,
                started_at: 1,
                duration_ms: 5_000,
            })
            .unwrap();
        let dir = paths.meeting_dir(ID);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mixed.opus"), b"x").unwrap();
        std::fs::write(dir.join(WAVEFORM_FILE), b"[0.1,1.0,0.5]").unwrap();
        (Library::new(store, paths), tmp)
    }

    #[test]
    fn detail_includes_waveform_and_audio() {
        let (lib, _tmp) = library();
        let d = lib.get(ID).unwrap().unwrap();
        assert_eq!(d.peaks, vec![0.1, 1.0, 0.5]);
        assert!(d.has_audio);
        assert_eq!(d.meeting.duration_ms, 5_000);
        assert!(lib.get("nope").unwrap().is_none());
    }

    #[test]
    fn rename_and_delete_with_files() {
        let (lib, _tmp) = library();
        assert!(matches!(
            lib.rename(ID, "  "),
            Err(LibraryError::EmptyTitle)
        ));
        lib.rename(ID, "Karoora").unwrap();
        assert_eq!(lib.list(None, 10).unwrap()[0].title, "Karoora");
        let dir = lib.get(ID).unwrap().unwrap().meeting.dir;
        lib.delete(ID).unwrap();
        assert!(!dir.exists());
        assert!(lib.list(None, 10).unwrap().is_empty());
        // Anything not shaped like a meeting ID is refused.
        assert!(lib.delete("../../etc").is_err());
    }
}
