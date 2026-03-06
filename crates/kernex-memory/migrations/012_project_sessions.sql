-- Project-scoped CLI sessions (replaces in-memory HashMap).
CREATE TABLE IF NOT EXISTS project_sessions (
    id              TEXT PRIMARY KEY,
    channel         TEXT NOT NULL,
    sender_id       TEXT NOT NULL,
    project         TEXT NOT NULL DEFAULT '',
    session_id      TEXT NOT NULL,
    parent_project  TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(channel, sender_id, project)
);
CREATE INDEX IF NOT EXISTS idx_project_sessions_lookup
    ON project_sessions(channel, sender_id, project);

-- Scope conversations to projects.
ALTER TABLE conversations ADD COLUMN project TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_conversations_project
    ON conversations(channel, sender_id, project, status, last_activity);
