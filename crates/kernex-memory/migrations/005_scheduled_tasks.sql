CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id           TEXT PRIMARY KEY,
    channel      TEXT NOT NULL,
    sender_id    TEXT NOT NULL,
    reply_target TEXT NOT NULL,
    description  TEXT NOT NULL,
    due_at       TEXT NOT NULL,
    repeat       TEXT,
    status       TEXT NOT NULL DEFAULT 'pending',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    delivered_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due ON scheduled_tasks(status, due_at);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_sender ON scheduled_tasks(sender_id, status);
