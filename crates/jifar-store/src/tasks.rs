//! Tasks from meeting notes (FR-AI-08, spec D12): a local checklist.
//!
//! Validation and ID generation happen in `jifar-core::tasks`; this module
//! only stores rows.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, Store, StoreError, fts_query, now_ms};

/// One task, with its meeting's title and the start of its first source
/// segment (for "jump to source").
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub meeting_id: Option<String>,
    pub meeting_title: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: bool,
    pub done_at: Option<i64>,
    /// `action_item`, `decision`, `key_point`, `summary`, `transcript` or
    /// `manual`.
    pub source_kind: String,
    pub source_version: Option<i64>,
    /// JSON array of segment IDs.
    pub segment_ids_json: String,
    pub source_start_ms: Option<i64>,
    pub match_key: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTaskRow {
    pub id: String,
    pub meeting_id: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub source_kind: String,
    pub source_version: Option<i64>,
    pub segment_ids_json: String,
    pub match_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFilter {
    Open,
    Done,
    All,
}

/// Fields to change; `None` leaves a field alone. For `owner` and `due`,
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    pub text: Option<String>,
    pub owner: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub done: Option<bool>,
}

const SELECT: &str = "SELECT t.id, t.meeting_id, m.title, t.text, t.owner, t.due, t.done,
        t.done_at, t.source_kind, t.source_version, t.segment_ids,
        (SELECT s.start_ms FROM segments s
          WHERE s.meeting_id = t.meeting_id AND s.id = json_extract(t.segment_ids, '$[0]')),
        t.match_key, t.created_at
     FROM tasks t LEFT JOIN meetings m ON m.id = t.meeting_id";

fn task_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: r.get(0)?,
        meeting_id: r.get(1)?,
        meeting_title: r.get(2)?,
        text: r.get(3)?,
        owner: r.get(4)?,
        due: r.get(5)?,
        done: r.get::<_, i64>(6)? != 0,
        done_at: r.get(7)?,
        source_kind: r.get(8)?,
        source_version: r.get(9)?,
        segment_ids_json: r.get(10)?,
        source_start_ms: r.get(11)?,
        match_key: r.get(12)?,
        created_at: r.get(13)?,
    })
}

fn get_task(c: &Connection, id: &str) -> Result<TaskRow> {
    c.query_row(&format!("{SELECT} WHERE t.id = ?1"), params![id], task_row)
        .optional()?
        .ok_or_else(|| StoreError::TaskNotFound(id.to_owned()))
}

impl Store {
    /// Open tasks first, then newest; a batch added together keeps its order.
    pub fn list_tasks(
        &self,
        filter: TaskFilter,
        meeting_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<TaskRow>> {
        let meeting_id = meeting_id.map(str::to_owned);
        let query = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(fts_query);
        let done: Option<i64> = match filter {
            TaskFilter::Open => Some(0),
            TaskFilter::Done => Some(1),
            TaskFilter::All => None,
        };
        self.call(move |c| {
            let rows = c
                .prepare(&format!(
                    "{SELECT}
                     WHERE (?1 IS NULL OR t.meeting_id = ?1)
                       AND (?2 IS NULL OR t.done = ?2)
                       AND (?3 IS NULL OR t.rowid IN
                            (SELECT rowid FROM tasks_fts WHERE tasks_fts MATCH ?3))
                     ORDER BY t.done, t.created_at DESC, t.rowid"
                ))?
                .query_map(params![meeting_id, done, query], task_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Adds tasks in one transaction and returns one row per item, in order.
    /// An item already added from the same notes entry (same meeting, kind
    /// and match key) returns the existing task. Fails with `NotFound` if a
    /// meeting doesn't exist.
    pub fn add_tasks(&self, items: Vec<NewTaskRow>) -> Result<Vec<TaskRow>> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let now = now_ms();
            let mut ids = Vec::with_capacity(items.len());
            for t in items {
                if let Some(m) = &t.meeting_id {
                    let exists = tx
                        .query_row("SELECT 1 FROM meetings WHERE id = ?1", params![m], |_| {
                            Ok(())
                        })
                        .optional()?
                        .is_some();
                    if !exists {
                        return Err(StoreError::NotFound(m.clone()));
                    }
                }
                let n = tx.execute(
                    "INSERT OR IGNORE INTO tasks (id, meeting_id, text, owner, due, source_kind,
                       source_version, segment_ids, match_key, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    params![
                        t.id,
                        t.meeting_id,
                        t.text,
                        t.owner,
                        t.due,
                        t.source_kind,
                        t.source_version,
                        t.segment_ids_json,
                        t.match_key,
                        now
                    ],
                )?;
                let id = if n == 1 {
                    t.id
                } else {
                    tx.query_row(
                        "SELECT id FROM tasks
                         WHERE meeting_id = ?1 AND source_kind = ?2 AND match_key = ?3",
                        params![t.meeting_id, t.source_kind, t.match_key],
                        |r| r.get(0),
                    )?
                };
                ids.push(id);
            }
            let rows = ids
                .iter()
                .map(|id| get_task(&tx, id))
                .collect::<Result<Vec<_>>>()?;
            tx.commit()?;
            Ok(rows)
        })
    }

    pub fn update_task(&self, id: &str, patch: TaskPatch) -> Result<TaskRow> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            get_task(&tx, &id)?;
            let now = now_ms();
            if let Some(text) = patch.text {
                tx.execute(
                    "UPDATE tasks SET text = ?2 WHERE id = ?1",
                    params![id, text],
                )?;
            }
            if let Some(owner) = patch.owner {
                tx.execute(
                    "UPDATE tasks SET owner = ?2 WHERE id = ?1",
                    params![id, owner],
                )?;
            }
            if let Some(due) = patch.due {
                tx.execute("UPDATE tasks SET due = ?2 WHERE id = ?1", params![id, due])?;
            }
            if let Some(done) = patch.done {
                tx.execute(
                    "UPDATE tasks SET done = ?2,
                       done_at = CASE WHEN ?2 = 1 THEN COALESCE(done_at, ?3) ELSE NULL END
                     WHERE id = ?1",
                    params![id, i64::from(done), now],
                )?;
            }
            tx.execute(
                "UPDATE tasks SET updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            let row = get_task(&tx, &id)?;
            tx.commit()?;
            Ok(row)
        })
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            if c.execute("DELETE FROM tasks WHERE id = ?1", params![id])? == 0 {
                return Err(StoreError::TaskNotFound(id));
            }
            Ok(())
        })
    }

    /// `(open, done)`.
    pub fn task_counts(&self) -> Result<(i64, i64)> {
        self.call(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(SUM(done = 0), 0), COALESCE(SUM(done = 1), 0) FROM tasks",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, SegmentRow, Status};

    fn store_with_meeting() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(Meeting {
            id: "m".into(),
            title: "Walgahii".into(),
            language: "om".into(),
            status: Status::Ready,
            started_at: 1,
            duration_ms: 0,
        })
        .unwrap();
        s.add_segments(
            "m",
            vec![SegmentRow {
                id: "m-s1".into(),
                track: "system".into(),
                start_ms: 134_000,
                end_ms: 136_000,
                text: "Gabaasa nan erga.".into(),
                edited: false,
            }],
        )
        .unwrap();
        s
    }

    fn item(id: &str, meeting: Option<&str>, kind: &str, text: &str) -> NewTaskRow {
        NewTaskRow {
            id: id.into(),
            meeting_id: meeting.map(Into::into),
            text: text.into(),
            owner: None,
            due: None,
            source_kind: kind.into(),
            source_version: Some(1),
            segment_ids_json: r#"["m-s1"]"#.into(),
            match_key: text.into(),
        }
    }

    #[test]
    fn add_skips_duplicates_from_notes_but_not_manual_tasks() {
        let s = store_with_meeting();
        let first = s
            .add_tasks(vec![
                item("t1", Some("m"), "action_item", "Gabaasa erguu"),
                item("t2", Some("m"), "decision", "Gabaasa erguu"),
            ])
            .unwrap();
        assert_eq!(first[0].meeting_title.as_deref(), Some("Walgahii"));
        assert_eq!(first[0].source_start_ms, Some(134_000));
        // Same item again (e.g. after regenerating notes): the existing task.
        let again = s
            .add_tasks(vec![item("t3", Some("m"), "action_item", "Gabaasa erguu")])
            .unwrap();
        assert_eq!(again[0].id, "t1");
        // Typed by hand: repeats are allowed.
        s.add_tasks(vec![
            item("t4", Some("m"), "manual", "x"),
            item("t5", Some("m"), "manual", "x"),
            item("t6", None, "manual", "x"),
        ])
        .unwrap();
        assert_eq!(s.task_counts().unwrap(), (5, 0));
        // Unknown meeting: nothing is added.
        assert!(matches!(
            s.add_tasks(vec![item("t7", Some("nope"), "action_item", "y")]),
            Err(StoreError::NotFound(_))
        ));
        assert_eq!(s.task_counts().unwrap(), (5, 0));
    }

    #[test]
    fn update_done_and_filters() {
        let s = store_with_meeting();
        s.add_tasks(vec![
            item("a", Some("m"), "action_item", "Gabaasa erguu"),
            item("b", Some("m"), "action_item", "Baajata qopheessuu"),
        ])
        .unwrap();
        let t = s
            .update_task(
                "a",
                TaskPatch {
                    done: Some(true),
                    owner: Some(Some("Caalaa".into())),
                    text: Some("Gabaasa ji'aa erguu".into()),
                    ..TaskPatch::default()
                },
            )
            .unwrap();
        assert!(t.done && t.done_at.is_some());
        assert_eq!(t.owner.as_deref(), Some("Caalaa"));
        // The match key is fixed, so the notes still show it as added.
        assert_eq!(t.match_key, "Gabaasa erguu");

        let ids = |f| -> Vec<String> {
            s.list_tasks(f, None, None)
                .unwrap()
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        assert_eq!(ids(TaskFilter::Open), vec!["b"]);
        assert_eq!(ids(TaskFilter::Done), vec!["a"]);
        // Open first, then done; a batch keeps its order.
        assert_eq!(ids(TaskFilter::All), vec!["b", "a"]);

        let t = s
            .update_task(
                "a",
                TaskPatch {
                    done: Some(false),
                    owner: Some(None),
                    ..TaskPatch::default()
                },
            )
            .unwrap();
        assert!(!t.done && t.done_at.is_none() && t.owner.is_none());
        assert!(matches!(
            s.update_task("zz", TaskPatch::default()),
            Err(StoreError::TaskNotFound(_))
        ));
    }

    #[test]
    fn search_keeps_qubee_words_whole_and_follows_edits() {
        let s = store_with_meeting();
        s.add_tasks(vec![
            item("a", Some("m"), "action_item", "Gabaasni ba'e"),
            item("b", Some("m"), "action_item", "Gabaasni bae"),
        ])
        .unwrap();
        let ids = |q| -> Vec<String> {
            s.list_tasks(TaskFilter::All, Some("m"), Some(q))
                .unwrap()
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        assert_eq!(ids("ba'e"), vec!["a"]);
        assert_eq!(ids("bae"), vec!["b"]);
        s.update_task(
            "b",
            TaskPatch {
                text: Some("Baajata".into()),
                ..TaskPatch::default()
            },
        )
        .unwrap();
        assert!(ids("bae").is_empty());
        assert_eq!(ids("baaj"), vec!["b"]);
        // Meeting search finds task words too.
        assert_eq!(s.list_meetings(Some("baajata"), 10).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_meeting_deletes_its_tasks() {
        let s = store_with_meeting();
        s.add_tasks(vec![
            item("a", Some("m"), "action_item", "x"),
            item("b", None, "manual", "y"),
        ])
        .unwrap();
        s.delete("m").unwrap();
        let left: Vec<String> = s
            .list_tasks(TaskFilter::All, None, None)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(left, vec!["b"]);
        s.delete_task("b").unwrap();
        assert!(matches!(
            s.delete_task("b"),
            Err(StoreError::TaskNotFound(_))
        ));
        assert_eq!(s.task_counts().unwrap(), (0, 0));
    }
}
