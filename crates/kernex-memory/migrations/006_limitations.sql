CREATE TABLE IF NOT EXISTS limitations (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    description   TEXT NOT NULL,
    proposed_plan TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'open',
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at   TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_limitations_title ON limitations(title COLLATE NOCASE);
