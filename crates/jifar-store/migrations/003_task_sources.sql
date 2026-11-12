-- Tasks can come from text selected in the summary or the transcript
-- (FR-AI-08). SQLite can't change a CHECK constraint in place, so the table
-- is rebuilt with the same rows and rowids (tasks_fts keys on rowid, so the
-- search index stays valid).

CREATE TABLE tasks_new (
    id             TEXT PRIMARY KEY,
    meeting_id     TEXT REFERENCES meetings (id) ON DELETE CASCADE,
    text           TEXT NOT NULL,
    owner          TEXT,
    due            TEXT,
    done           INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0, 1)),
    done_at        INTEGER,
    source_kind    TEXT NOT NULL
                   CHECK (source_kind IN ('action_item', 'decision', 'key_point',
                                          'summary', 'transcript', 'manual')),
    source_version INTEGER,
    segment_ids    TEXT NOT NULL DEFAULT '[]',
    match_key      TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
INSERT INTO tasks_new (rowid, id, meeting_id, text, owner, due, done, done_at, source_kind,
                       source_version, segment_ids, match_key, created_at, updated_at)
    SELECT rowid, id, meeting_id, text, owner, due, done, done_at, source_kind,
           source_version, segment_ids, match_key, created_at, updated_at
    FROM tasks;
-- Drops its indexes and triggers too.
DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

CREATE INDEX tasks_open ON tasks (done, created_at DESC);
CREATE INDEX tasks_meeting ON tasks (meeting_id);
CREATE UNIQUE INDEX tasks_from_notes ON tasks (meeting_id, source_kind, match_key)
    WHERE meeting_id IS NOT NULL AND source_kind <> 'manual';

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
