//! SQLite storage on a dedicated thread, numbered migrations and FTS5 (SPEC §7).
//!
//! Schema changes only via new numbered migrations; never edit an applied one.
//! FTS uses `tokenchars "'"` so Qubee words like `ba'e` stay whole.
//!
//! Every query runs on one thread that owns the connection; callers send a
//! closure and block for its result. In async code, call from a blocking
//! task. WAL mode keeps writes cheap and crash-safe.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use tracing::{info, warn};

mod tasks;

pub use tasks::{NewTaskRow, TaskFilter, TaskPatch, TaskRow};

const MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("../migrations/001_initial.sql")),
    M::up(include_str!("../migrations/002_tasks.sql")),
    M::up(include_str!("../migrations/003_task_sources.sql")),
];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("database thread stopped")]
    Closed,
    #[error("meeting {0} not found")]
    NotFound(String),
    #[error("task {0} not found")]
    TaskNotFound(String),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Recording,
    Processing,
    Ready,
    NeedsAttention,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::NeedsAttention => "needs_attention",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "recording" => Self::Recording,
            "processing" => Self::Processing,
            "ready" => Self::Ready,
            _ => Self::NeedsAttention,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub language: String,
    pub status: Status,
    pub started_at: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackRow {
    /// `mic`, `system` or `mixed`.
    pub kind: String,
    pub path: String,
    pub duration_ms: i64,
    pub start_offset_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRow {
    pub id: String,
    /// `mic` or `system`.
    pub track: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub edited: bool,
}

/// One saved version of a meeting's notes (FR-AI-04).
#[derive(Debug, Clone, PartialEq)]
pub struct NotesRow {
    pub version: i64,
    /// `ai` or `user`.
    pub source: String,
    pub provider: Option<String>,
    pub language: String,
    /// `MeetingNotes` JSON.
    pub body_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub tracks: Vec<TrackRow>,
    pub segments: Vec<SegmentRow>,
    /// Latest notes version, if any.
    pub notes: Option<NotesRow>,
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle to the database thread. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    tx: mpsc::Sender<Job>,
}

impl Store {
    /// Opens (or creates) the database at `path` and migrates it.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                StoreError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        Self::start(Connection::open(path)?, true)
    }

    /// For tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::start(Connection::open_in_memory()?, false)
    }

    fn start(mut conn: Connection, wal: bool) -> Result<Self> {
        if wal {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Migrations::from_slice(MIGRATIONS).to_latest(&mut conn)?;
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("jifar-db".into())
            .spawn(move || {
                for job in rx {
                    job(&mut conn);
                }
            })
            .map_err(|e| {
                StoreError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        Ok(Self { tx })
    }

    /// Runs `f` on the database thread and waits for its result.
    pub fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        self.tx
            .send(Box::new(move |conn| {
                let _ = done_tx.send(f(conn));
            }))
            .map_err(|_| StoreError::Closed)?;
        done_rx.recv().map_err(|_| StoreError::Closed)?
    }

    // ---- Meetings ---------------------------------------------------------

    pub fn create_meeting(&self, m: Meeting) -> Result<()> {
        self.call(move |c| {
            let now = now_ms();
            c.execute(
                "INSERT INTO meetings (id, title, language, status, started_at, duration_ms, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![m.id, m.title, m.language, m.status.as_str(), m.started_at, m.duration_ms, now],
            )?;
            Ok(())
        })
    }

    /// Records the end of a recording: duration, status and its tracks.
    pub fn finish_recording(
        &self,
        id: &str,
        duration_ms: i64,
        status: Status,
        tracks: Vec<TrackRow>,
    ) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute(
                "UPDATE meetings SET duration_ms = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, duration_ms, status.as_str(), now_ms()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(id));
            }
            for t in tracks {
                tx.execute(
                    "INSERT INTO tracks (id, meeting_id, kind, path, duration_ms, start_offset_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (meeting_id, kind) DO UPDATE SET
                       path = excluded.path, duration_ms = excluded.duration_ms,
                       start_offset_ms = excluded.start_offset_ms",
                    params![
                        format!("{id}-{}", t.kind),
                        id,
                        t.kind,
                        t.path,
                        t.duration_ms,
                        t.start_offset_ms
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn set_status(&self, id: &str, status: Status) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE meetings SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, status.as_str(), now_ms()],
            )?;
            Ok(())
        })
    }

    /// Transcription finished: `ready`, or `needs_attention` if chunks
    /// failed. A meeting already flagged (e.g. audio lost to overruns) stays
    /// flagged.
    pub fn finish_transcription(&self, id: &str, failed: usize) -> Result<()> {
        let id = id.to_owned();
        let failed = i64::try_from(failed).unwrap_or(i64::MAX);
        self.call(move |c| {
            c.execute(
                "UPDATE meetings SET updated_at = ?3, status = CASE
                   WHEN ?2 > 0 THEN 'needs_attention'
                   WHEN status = 'processing' THEN 'ready'
                   ELSE status END
                 WHERE id = ?1",
                params![id, failed, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<()> {
        let (id, title) = (id.to_owned(), title.trim().to_owned());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now_ms()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(id));
            }
            Ok(())
        })
    }

    /// Deletes the meeting and everything that belongs to it in the database.
    /// The caller removes its files.
    pub fn delete(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            c.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Newest first. `query` matches titles (substring), and transcript and
    /// task text (full-text, whole Qubee words).
    pub fn list_meetings(&self, query: Option<&str>, limit: u32) -> Result<Vec<Meeting>> {
        let query = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_owned);
        self.call(move |c| {
            let rows = match query {
                None => {
                    let mut st = c.prepare(
                        "SELECT id, title, language, status, started_at, duration_ms
                         FROM meetings ORDER BY started_at DESC LIMIT ?1",
                    )?;
                    st.query_map(params![limit], meeting_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
                Some(q) => {
                    let mut st = c.prepare(
                        "SELECT id, title, language, status, started_at, duration_ms FROM meetings
                         WHERE title LIKE '%' || ?1 || '%'
                            OR id IN (SELECT s.meeting_id FROM segments_fts f
                                      JOIN segments s ON s.rowid = f.rowid
                                      WHERE segments_fts MATCH ?2)
                            OR id IN (SELECT t.meeting_id FROM tasks_fts f
                                      JOIN tasks t ON t.rowid = f.rowid
                                      WHERE tasks_fts MATCH ?2)
                         ORDER BY started_at DESC LIMIT ?3",
                    )?;
                    st.query_map(params![q, fts_query(&q), limit], meeting_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
            };
            Ok(rows)
        })
    }

    pub fn get_meeting(&self, id: &str) -> Result<Option<MeetingDetail>> {
        let id = id.to_owned();
        self.call(move |c| {
            let Some(meeting) = c
                .query_row(
                    "SELECT id, title, language, status, started_at, duration_ms
                     FROM meetings WHERE id = ?1",
                    params![id],
                    meeting_row,
                )
                .optional()?
            else {
                return Ok(None);
            };
            let tracks = c
                .prepare(
                    "SELECT kind, path, duration_ms, start_offset_ms FROM tracks
                     WHERE meeting_id = ?1 ORDER BY kind",
                )?
                .query_map(params![id], |r| {
                    Ok(TrackRow {
                        kind: r.get(0)?,
                        path: r.get(1)?,
                        duration_ms: r.get(2)?,
                        start_offset_ms: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let segments = c
                .prepare(
                    "SELECT id, track, start_ms, end_ms, text, edited FROM segments
                     WHERE meeting_id = ?1 ORDER BY start_ms, track",
                )?
                .query_map(params![id], |r| {
                    Ok(SegmentRow {
                        id: r.get(0)?,
                        track: r.get(1)?,
                        start_ms: r.get(2)?,
                        end_ms: r.get(3)?,
                        text: r.get(4)?,
                        edited: r.get::<_, i64>(5)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let notes = latest_notes(c, &id)?;
            Ok(Some(MeetingDetail {
                meeting,
                tracks,
                segments,
                notes,
            }))
        })
    }

    // ---- Segments ---------------------------------------------------------

    /// Adds segments; existing IDs are left alone, so a re-run never
    /// duplicates them or overwrites a user's edit.
    pub fn add_segments(&self, meeting_id: &str, segments: Vec<SegmentRow>) -> Result<()> {
        let meeting_id = meeting_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare(
                    "INSERT OR IGNORE INTO segments (id, meeting_id, track, start_ms, end_ms, text)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for s in segments {
                    st.execute(params![
                        s.id, meeting_id, s.track, s.start_ms, s.end_ms, s.text
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    // ---- Notes --------------------------------------------------------------

    /// Saves a new notes version (never overwrites an earlier one, so a
    /// user's own version survives regeneration). Returns the version.
    pub fn add_notes(
        &self,
        meeting_id: &str,
        source: &str,
        provider: Option<&str>,
        language: &str,
        body_json: &str,
    ) -> Result<i64> {
        let (meeting_id, source, provider, language, body) = (
            meeting_id.to_owned(),
            source.to_owned(),
            provider.map(str::to_owned),
            language.to_owned(),
            body_json.to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            let version: i64 = tx.query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM notes WHERE meeting_id = ?1",
                params![meeting_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO notes (id, meeting_id, version, source, provider, language, body_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    format!("{meeting_id}-notes-{version}"),
                    meeting_id,
                    version,
                    source,
                    provider,
                    language,
                    body,
                    now_ms()
                ],
            )?;
            tx.commit()?;
            Ok(version)
        })
    }

    // ---- Startup ----------------------------------------------------------

    /// After a crash or kill: meetings still marked `recording` lost their
    /// recorder, so they need attention (their files are kept and can be
    /// transcribed). Returns how many were fixed.
    pub fn recover_interrupted(&self) -> Result<usize> {
        let n = self.call(|c| {
            Ok(c.execute(
                "UPDATE meetings SET status = 'needs_attention', updated_at = ?1
                 WHERE status = 'recording'",
                params![now_ms()],
            )?)
        })?;
        if n > 0 {
            warn!(
                meetings = n,
                "recovered meetings interrupted while recording"
            );
        }
        Ok(n)
    }

    /// Meetings whose transcription was cut short (the app closed while it
    /// ran). Their audio is on disk; transcription can be resumed.
    pub fn unfinished_transcriptions(&self) -> Result<Vec<String>> {
        self.call(|c| {
            let ids = c
                .prepare("SELECT id FROM meetings WHERE status = 'processing'")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(ids)
        })
    }
}

fn latest_notes(c: &Connection, meeting_id: &str) -> Result<Option<NotesRow>> {
    Ok(c.query_row(
        "SELECT version, source, provider, language, body_json, created_at FROM notes
         WHERE meeting_id = ?1 ORDER BY version DESC LIMIT 1",
        params![meeting_id],
        |r| {
            Ok(NotesRow {
                version: r.get(0)?,
                source: r.get(1)?,
                provider: r.get(2)?,
                language: r.get(3)?,
                body_json: r.get(4)?,
                created_at: r.get(5)?,
            })
        },
    )
    .optional()?)
}

fn meeting_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: r.get(0)?,
        title: r.get(1)?,
        language: r.get(2)?,
        status: Status::parse(&r.get::<_, String>(3)?),
        started_at: r.get(4)?,
        duration_ms: r.get(5)?,
    })
}

/// A safe FTS5 query: each word quoted (so `ba'e`, `-` or `"` in the search
/// box never become FTS syntax), the last one as a prefix.
pub(crate) fn fts_query(q: &str) -> String {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    match words.split_last() {
        Some((last, rest)) => {
            let mut parts = rest.to_vec();
            parts.push(format!("{last}*"));
            parts.join(" ")
        }
        None => "\"\"".into(),
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Logs the schema version once at startup.
pub fn log_version(store: &Store) {
    if let Ok(v) =
        store.call(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?))
    {
        info!(schema = v, "database ready");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(id: &str, title: &str, started_at: i64) -> Meeting {
        Meeting {
            id: id.into(),
            title: title.into(),
            language: "om".into(),
            status: Status::Recording,
            started_at,
            duration_ms: 0,
        }
    }

    fn seg(id: &str, track: &str, start: i64, text: &str) -> SegmentRow {
        SegmentRow {
            id: id.into(),
            track: track.into(),
            start_ms: start,
            end_ms: start + 1_000,
            text: text.into(),
            edited: false,
        }
    }

    #[test]
    fn migrations_are_valid() {
        assert!(Migrations::from_slice(MIGRATIONS).validate().is_ok());
    }

    #[test]
    fn meeting_lifecycle() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(meeting("m1", "Walgahii", 10)).unwrap();
        s.finish_recording(
            "m1",
            65_000,
            Status::Processing,
            vec![TrackRow {
                kind: "mic".into(),
                path: "/x/mic.opus".into(),
                duration_ms: 65_000,
                start_offset_ms: 12,
            }],
        )
        .unwrap();
        s.add_segments(
            "m1",
            vec![
                seg("m1-system-0000-0", "system", 3_000, "Galatoomi."),
                seg("m1-mic-0000-0", "mic", 1_000, "Akkam bultan."),
            ],
        )
        .unwrap();
        // Re-adding the same IDs (a re-run) changes nothing.
        s.add_segments("m1", vec![seg("m1-mic-0000-0", "mic", 1_000, "changed")])
            .unwrap();
        s.finish_transcription("m1", 0).unwrap();
        s.rename("m1", "  Karoora  ").unwrap();

        let d = s.get_meeting("m1").unwrap().unwrap();
        assert_eq!(d.meeting.title, "Karoora");
        assert_eq!(d.meeting.status, Status::Ready);
        assert_eq!(d.meeting.duration_ms, 65_000);
        assert_eq!(d.tracks[0].start_offset_ms, 12);
        let texts: Vec<&str> = d.segments.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["Akkam bultan.", "Galatoomi."]);

        s.delete("m1").unwrap();
        assert!(s.get_meeting("m1").unwrap().is_none());
        // Segments went with it (and out of the search index).
        assert!(s.list_meetings(Some("Galatoomi"), 10).unwrap().is_empty());
    }

    #[test]
    fn list_is_newest_first_and_search_keeps_qubee_words_whole() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(meeting("a", "Karoora kurmaana", 100))
            .unwrap();
        s.create_meeting(meeting("b", "Walgahii garee", 200))
            .unwrap();
        s.add_segments("a", vec![seg("a-1", "mic", 0, "Gabaasni ba'e har'a.")])
            .unwrap();
        s.add_segments("b", vec![seg("b-1", "mic", 0, "Gabaasni bae hin jiru.")])
            .unwrap();

        let ids = |q: Option<&str>| -> Vec<String> {
            s.list_meetings(q, 10)
                .unwrap()
                .into_iter()
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(ids(None), vec!["b", "a"]);
        assert_eq!(ids(Some("garee")), vec!["b"]); // title
        // "ba'e" is one token: it matches only meeting a, never "bae".
        assert_eq!(ids(Some("ba'e")), vec!["a"]);
        assert_eq!(ids(Some("bae")), vec!["b"]);
        // Prefix on the last word, case-insensitive.
        assert_eq!(ids(Some("gabaas")), vec!["b", "a"]);
        // FTS syntax characters are just text.
        assert!(ids(Some("\"AND -( *")).is_empty());
    }

    #[test]
    fn notes_are_versioned_and_searchable() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(meeting("m", "x", 1)).unwrap();
        assert!(s.get_meeting("m").unwrap().unwrap().notes.is_none());
        assert_eq!(
            s.add_notes(
                "m",
                "ai",
                Some("addis-llm"),
                "om",
                r#"{"summary":"Baajata ba'e"}"#
            )
            .unwrap(),
            1
        );
        assert_eq!(
            s.add_notes(
                "m",
                "ai",
                Some("addis-llm"),
                "en",
                r#"{"summary":"Budget"}"#
            )
            .unwrap(),
            2
        );
        let n = s.get_meeting("m").unwrap().unwrap().notes.unwrap();
        assert_eq!((n.version, n.language.as_str()), (2, "en"));
        // Deleting the meeting removes its notes too.
        s.delete("m").unwrap();
        assert_eq!(
            s.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get::<_, i64>(0))?))
                .unwrap(),
            0
        );
    }

    #[test]
    fn interrupted_recordings_are_recovered() {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(meeting("crashed", "x", 1)).unwrap();
        s.create_meeting(meeting("half", "y", 2)).unwrap();
        s.set_status("half", Status::Processing).unwrap();
        assert_eq!(s.recover_interrupted().unwrap(), 1);
        assert_eq!(
            s.get_meeting("crashed").unwrap().unwrap().meeting.status,
            Status::NeedsAttention
        );
        assert_eq!(s.unfinished_transcriptions().unwrap(), vec!["half"]);
        // A flagged meeting stays flagged when transcription succeeds.
        s.finish_transcription("crashed", 0).unwrap();
        assert_eq!(
            s.get_meeting("crashed").unwrap().unwrap().meeting.status,
            Status::NeedsAttention
        );
        s.finish_transcription("half", 2).unwrap();
        assert_eq!(
            s.get_meeting("half").unwrap().unwrap().meeting.status,
            Status::NeedsAttention
        );
    }

    #[test]
    fn task_sources_migration_keeps_rows_and_search() {
        let mut c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        let m = Migrations::from_slice(MIGRATIONS);
        m.to_version(&mut c, 2).unwrap();
        c.execute_batch(
            "INSERT INTO meetings (id, title, language, status, started_at, duration_ms, created_at, updated_at)
               VALUES ('m', 't', 'om', 'ready', 1, 0, 1, 1);
             INSERT INTO tasks (id, meeting_id, text, source_kind, match_key, created_at, updated_at)
               VALUES ('t1', 'm', 'Gabaasa ba''e erguu', 'action_item', 'Gabaasa ba''e erguu', 1, 1);",
        )
        .unwrap();
        m.to_latest(&mut c).unwrap();
        // The old row is still found by search, and the new kinds are allowed.
        let hit: String = c
            .query_row(
                "SELECT t.id FROM tasks_fts f JOIN tasks t ON t.rowid = f.rowid
                 WHERE tasks_fts MATCH '\"ba''e\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, "t1");
        c.execute(
            "INSERT INTO tasks (id, meeting_id, text, source_kind, match_key, created_at, updated_at)
             VALUES ('t2', 'm', 'x', 'summary', 'x', 2, 2)",
            [],
        )
        .unwrap();
        // Triggers were recreated: a new row is searchable, cascade still works.
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM tasks_fts WHERE tasks_fts MATCH 'x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        c.execute("DELETE FROM meetings WHERE id = 'm'", [])
            .unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn data_survives_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jifar.db");
        {
            let s = Store::open(&path).unwrap();
            s.create_meeting(meeting("m", "kept", 1)).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.list_meetings(None, 10).unwrap()[0].title, "kept");
    }
}
