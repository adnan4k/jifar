-- Tasks from meeting notes (FR-AI-08, spec D12). A task copies its text when
-- it is added, so regenerating notes never changes it.

CREATE TABLE tasks (
    id             TEXT PRIMARY KEY,
    -- NULL for a task typed by hand without a meeting.
    meeting_id     TEXT REFERENCES meetings (id) ON DELETE CASCADE,
    text           TEXT NOT NULL,
    owner          TEXT,
    due            TEXT,
    done           INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0, 1)),
    done_at        INTEGER,
    source_kind    TEXT NOT NULL
                   CHECK (source_kind IN ('action_item', 'decision', 'key_point', 'manual')),
    source_version INTEGER,
    -- JSON array of segment IDs, for "jump to source".
    segment_ids    TEXT NOT NULL DEFAULT '[]',
    -- The source text, trimmed with whitespace collapsed (case and Qubee
    -- apostrophes kept). Fixed at creation, so editing a task keeps it
    -- marked as added in the notes.
    match_key      TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX tasks_open ON tasks (done, created_at DESC);
CREATE INDEX tasks_meeting ON tasks (meeting_id);
-- One task per notes item; tasks typed by hand may repeat.
CREATE UNIQUE INDEX tasks_from_notes ON tasks (meeting_id, source_kind, match_key)
    WHERE meeting_id IS NOT NULL AND source_kind <> 'manual';

CREATE VIRTUAL TABLE tasks_fts USING fts5 (
    text,
    content = 'tasks',
    content_rowid = 'rowid',
    tokenize = "unicode61 tokenchars ''''"
);
CREATE TRIGGER tasks_ai AFTER INSERT ON tasks BEGIN
    INSERT INTO tasks_fts (rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER tasks_ad AFTER DELETE ON tasks BEGIN
    INSERT INTO tasks_fts (tasks_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER tasks_au AFTER UPDATE OF text ON tasks BEGIN
    INSERT INTO tasks_fts (tasks_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO tasks_fts (rowid, text) VALUES (new.rowid, new.text);
END;
