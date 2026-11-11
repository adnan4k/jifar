-- Migration 001: initial schema (SPEC §7.2). Never edit once applied; add
-- a new numbered migration instead.
--
-- Times are integer ms: *_at columns are Unix epoch ms; start_ms/end_ms are
-- recording time (pauses excluded). IDs are UUIDv7 text unless noted.

CREATE TABLE meetings (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    language    TEXT NOT NULL DEFAULT 'om',
    status      TEXT NOT NULL
                CHECK (status IN ('recording', 'processing', 'ready', 'needs_attention')),
    started_at  INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX meetings_started ON meetings (started_at DESC);

CREATE TABLE tracks (
    id          TEXT PRIMARY KEY,
    meeting_id  TEXT NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('mic', 'system', 'mixed')),
    path        TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    -- Where the track's first sample sits on the recording clock.
    start_offset_ms INTEGER NOT NULL DEFAULT 0,
    UNIQUE (meeting_id, kind)
);

CREATE TABLE asr_jobs (
    id              TEXT PRIMARY KEY,
    meeting_id      TEXT NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    track           TEXT NOT NULL CHECK (track IN ('mic', 'system')),
    chunk_index     INTEGER NOT NULL,
    path            TEXT NOT NULL,
    start_ms        INTEGER NOT NULL,
    end_ms          INTEGER NOT NULL,
    provider        TEXT NOT NULL,
    state           TEXT NOT NULL
                    CHECK (state IN ('pending', 'in_flight', 'done', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER,
    last_error      TEXT,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX asr_jobs_meeting ON asr_jobs (meeting_id, state);

CREATE TABLE segments (
    -- Stable per meeting and chunk (`<meeting>-<track>-<chunk>-<n>`), so
    -- re-running a transcription never duplicates segments.
    id            TEXT PRIMARY KEY,
    meeting_id    TEXT NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    track         TEXT NOT NULL CHECK (track IN ('mic', 'system')),
    speaker       TEXT,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER NOT NULL,
    text          TEXT NOT NULL,
    original_text TEXT,
    edited        INTEGER NOT NULL DEFAULT 0 CHECK (edited IN (0, 1)),
    confidence    REAL,
    job_id        TEXT
);
CREATE INDEX segments_meeting ON segments (meeting_id, start_ms);

CREATE TABLE notes (
    id         TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    source     TEXT NOT NULL CHECK (source IN ('ai', 'user')),
    provider   TEXT,
    language   TEXT NOT NULL,
    body_json  TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (meeting_id, version)
);

-- Full-text search. unicode61 with the apostrophe as a token character, so
-- Qubee words like ba'e and mari'anna stay whole (never split or folded).
CREATE VIRTUAL TABLE segments_fts USING fts5 (
    text,
    content = 'segments',
    content_rowid = 'rowid',
    tokenize = "unicode61 tokenchars ''''"
);
CREATE TRIGGER segments_ai AFTER INSERT ON segments BEGIN
    INSERT INTO segments_fts (rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER segments_ad AFTER DELETE ON segments BEGIN
    INSERT INTO segments_fts (segments_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER segments_au AFTER UPDATE OF text ON segments BEGIN
    INSERT INTO segments_fts (segments_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO segments_fts (rowid, text) VALUES (new.rowid, new.text);
END;

CREATE VIRTUAL TABLE notes_fts USING fts5 (
    body_json,
    content = 'notes',
    content_rowid = 'rowid',
    tokenize = "unicode61 tokenchars ''''"
);
CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts (rowid, body_json) VALUES (new.rowid, new.body_json);
END;
CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts (notes_fts, rowid, body_json) VALUES ('delete', old.rowid, old.body_json);
END;
