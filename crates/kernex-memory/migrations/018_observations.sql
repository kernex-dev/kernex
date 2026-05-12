-- Typed observation log for the `kx mem save` write surface.
--
-- Per-DB scoping (sender_id, no project column) matches the existing
-- `facts` / `messages` discipline. The on-disk DB at
-- `~/.kx/projects/<name>/memory.db` is the project scope; intra-DB
-- scoping is by sender_id.
--
-- Soft-delete via `deleted_at` mirrors `017_soft_delete.sql`. The
-- agent's CC-9 invariant relies on the partial index for default-read
-- performance.
--
-- FTS5 mirror keeps title + the four structured fields searchable.
-- Triggers wire insert / update / delete so the FTS table stays in
-- sync with the base table automatically; consumers never write to
-- `observations_fts` directly.

CREATE TABLE IF NOT EXISTS observations (
    id           TEXT PRIMARY KEY,
    sender_id    TEXT NOT NULL,
    type         TEXT NOT NULL
                 CHECK (type IN (
                     'bugfix', 'decision', 'pattern', 'config',
                     'discovery', 'learning', 'architecture'
                 )),
    title        TEXT NOT NULL
                 CHECK (length(title) > 0),
    what         TEXT,
    why          TEXT,
    where_field  TEXT,
    learned      TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    deleted_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_observations_active
    ON observations (sender_id, created_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_observations_type_active
    ON observations (sender_id, type, created_at DESC)
    WHERE deleted_at IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    title,
    what,
    why,
    where_field,
    learned,
    content='observations',
    content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS observations_ai
    AFTER INSERT ON observations
    WHEN new.deleted_at IS NULL
BEGIN
    INSERT INTO observations_fts (rowid, title, what, why, where_field, learned)
    VALUES (new.rowid, new.title, new.what, new.why, new.where_field, new.learned);
END;

CREATE TRIGGER IF NOT EXISTS observations_au
    AFTER UPDATE ON observations
BEGIN
    INSERT INTO observations_fts (observations_fts, rowid, title, what, why, where_field, learned)
    VALUES ('delete', old.rowid, old.title, old.what, old.why, old.where_field, old.learned);
    INSERT INTO observations_fts (rowid, title, what, why, where_field, learned)
    SELECT new.rowid, new.title, new.what, new.why, new.where_field, new.learned
    WHERE new.deleted_at IS NULL;
END;

CREATE TRIGGER IF NOT EXISTS observations_ad
    AFTER DELETE ON observations
BEGIN
    INSERT INTO observations_fts (observations_fts, rowid, title, what, why, where_field, learned)
    VALUES ('delete', old.rowid, old.title, old.what, old.why, old.where_field, old.learned);
END;
