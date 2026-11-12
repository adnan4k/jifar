//! Tasks from meeting notes (FR-AI-08, spec D12): a local checklist of
//! follow-ups. A task copies the notes item's text when it is added, so
//! regenerating notes never changes it, and user edits are never overwritten.

use jifar_store::{NewTaskRow, Store, StoreError, TaskFilter, TaskPatch, TaskRow};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TasksError {
    #[error("{0}")]
    Store(StoreError),
    #[error("a task needs some text")]
    EmptyText,
    #[error("task {0} not found")]
    NotFound(String),
    #[error("meeting {0} not found")]
    MeetingNotFound(String),
}

impl From<StoreError> for TasksError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::TaskNotFound(id) => Self::NotFound(id),
            StoreError::NotFound(id) => Self::MeetingNotFound(id),
            other => Self::Store(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    ActionItem,
    Decision,
    KeyPoint,
    /// Text selected in the summary.
    Summary,
    /// Text selected in the transcript.
    Transcript,
    /// Typed by hand.
    Manual,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActionItem => "action_item",
            Self::Decision => "decision",
            Self::KeyPoint => "key_point",
            Self::Summary => "summary",
            Self::Transcript => "transcript",
            Self::Manual => "manual",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "action_item" => Self::ActionItem,
            "decision" => Self::Decision,
            "key_point" => Self::KeyPoint,
            "summary" => Self::Summary,
            "transcript" => Self::Transcript,
            _ => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub meeting_id: Option<String>,
    pub meeting_title: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: bool,
    pub done_at_ms: Option<i64>,
    pub source_kind: SourceKind,
    pub source_version: Option<i64>,
    pub segment_ids: Vec<String>,
    /// Start of the first source segment, if it still exists.
    pub source_start_ms: Option<i64>,
    /// See [`match_key`].
    pub match_key: String,
    pub created_at_ms: i64,
}

impl From<TaskRow> for Task {
    fn from(r: TaskRow) -> Self {
        Self {
            segment_ids: serde_json::from_str(&r.segment_ids_json).unwrap_or_default(),
            id: r.id,
            meeting_id: r.meeting_id,
            meeting_title: r.meeting_title,
            text: r.text,
            owner: r.owner,
            due: r.due,
            done: r.done,
            done_at_ms: r.done_at,
            source_kind: SourceKind::parse(&r.source_kind),
            source_version: r.source_version,
            source_start_ms: r.source_start_ms,
            match_key: r.match_key,
            created_at_ms: r.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    pub meeting_id: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub source_kind: SourceKind,
    pub source_version: Option<i64>,
    pub segment_ids: Vec<String>,
}

/// Changes to a task. `None` leaves a field alone; an empty `owner` or `due`
/// clears it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskChanges {
    pub text: Option<String>,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: Option<bool>,
}

/// How a notes item is recognised as already added: trimmed, whitespace
/// collapsed. Case and Qubee apostrophes are kept (`ba'e` ≠ `bae`). The UI
/// computes the same key.
pub fn match_key(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

#[derive(Clone)]
pub struct Tasks {
    store: Store,
}

impl Tasks {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn list(
        &self,
        filter: TaskFilter,
        meeting_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<Task>, TasksError> {
        Ok(self
            .store
            .list_tasks(filter, meeting_id, query)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Adds all items or none. Items already in tasks come back as the
    /// existing task.
    pub fn add(&self, items: Vec<NewTask>) -> Result<Vec<Task>, TasksError> {
        let rows = items
            .into_iter()
            .map(|t| {
                let text = t.text.trim().to_owned();
                if text.is_empty() {
                    return Err(TasksError::EmptyText);
                }
                Ok(NewTaskRow {
                    id: Uuid::now_v7().to_string(),
                    meeting_id: t.meeting_id,
                    match_key: match_key(&text),
                    text,
                    owner: clean(t.owner),
                    due: clean(t.due),
                    source_kind: t.source_kind.as_str().to_owned(),
                    source_version: t.source_version,
                    segment_ids_json: serde_json::to_string(&t.segment_ids)
                        .unwrap_or_else(|_| "[]".into()),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self
            .store
            .add_tasks(rows)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn update(&self, id: &str, changes: TaskChanges) -> Result<Task, TasksError> {
        let text = match changes.text.map(|t| t.trim().to_owned()) {
            Some(t) if t.is_empty() => return Err(TasksError::EmptyText),
            t => t,
        };
        let patch = TaskPatch {
            text,
            owner: changes.owner.map(|o| clean(Some(o))),
            due: changes.due.map(|d| clean(Some(d))),
            done: changes.done,
        };
        Ok(self.store.update_task(id, patch)?.into())
    }

    pub fn delete(&self, id: &str) -> Result<(), TasksError> {
        Ok(self.store.delete_task(id)?)
    }

    /// `(open, done)`.
    pub fn counts(&self) -> Result<(i64, i64), TasksError> {
        Ok(self.store.task_counts()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jifar_store::{Meeting, Status};

    fn tasks() -> Tasks {
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
        Tasks::new(s)
    }

    fn action(text: &str) -> NewTask {
        NewTask {
            meeting_id: Some("m".into()),
            text: text.into(),
            owner: Some("  Caalaa ".into()),
            due: Some("   ".into()),
            source_kind: SourceKind::ActionItem,
            source_version: Some(1),
            segment_ids: vec!["s2".into()],
        }
    }

    #[test]
    fn match_key_collapses_whitespace_and_keeps_qubee() {
        assert_eq!(
            match_key("  Gabaasa\n ji'aa   erguu "),
            "Gabaasa ji'aa erguu"
        );
        assert_ne!(match_key("ba'e"), match_key("bae"));
        assert_ne!(match_key("Baajata"), match_key("baajata"));
    }

    #[test]
    fn add_cleans_fields_and_survives_regenerated_notes() {
        let t = tasks();
        let added = t.add(vec![action("Gabaasa  erguu")]).unwrap();
        // Text is kept as written (trimmed); only the match key collapses.
        assert_eq!(added[0].text, "Gabaasa  erguu");
        assert_eq!(added[0].match_key, "Gabaasa erguu");
        assert_eq!(added[0].owner.as_deref(), Some("Caalaa"));
        assert_eq!(added[0].due, None);
        assert_eq!(added[0].segment_ids, vec!["s2"]);
        // A regenerated notes version with the same item (spacing aside).
        let mut again = action(" Gabaasa erguu ");
        again.source_version = Some(2);
        let second = t.add(vec![again]).unwrap();
        assert_eq!(second[0].id, added[0].id);
        assert_eq!(t.counts().unwrap(), (1, 0));
    }

    #[test]
    fn empty_text_is_rejected_and_nothing_is_added() {
        let t = tasks();
        assert!(matches!(
            t.add(vec![action("ok"), action("  ")]),
            Err(TasksError::EmptyText)
        ));
        assert_eq!(t.counts().unwrap(), (0, 0));
        let id = t.add(vec![action("ok")]).unwrap()[0].id.clone();
        assert!(matches!(
            t.update(
                &id,
                TaskChanges {
                    text: Some(" ".into()),
                    ..TaskChanges::default()
                }
            ),
            Err(TasksError::EmptyText)
        ));
    }

    #[test]
    fn update_clears_owner_with_empty_string() {
        let t = tasks();
        let id = t.add(vec![action("ok")]).unwrap()[0].id.clone();
        let u = t
            .update(
                &id,
                TaskChanges {
                    owner: Some(String::new()),
                    due: Some(" Jimaata ".into()),
                    done: Some(true),
                    ..TaskChanges::default()
                },
            )
            .unwrap();
        assert_eq!(u.owner, None);
        assert_eq!(u.due.as_deref(), Some("Jimaata"));
        assert!(u.done);
        assert!(matches!(t.delete("missing"), Err(TasksError::NotFound(_))));
        assert!(matches!(
            t.add(vec![NewTask {
                meeting_id: Some("gone".into()),
                ..action("x")
            }]),
            Err(TasksError::MeetingNotFound(_))
        ));
    }
}
