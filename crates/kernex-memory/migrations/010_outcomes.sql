-- Reward-based learning: raw outcomes (working memory) and distilled lessons (long-term memory).

CREATE TABLE IF NOT EXISTS outcomes (
    id        TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL DEFAULT (datetime('now')),
    sender_id TEXT NOT NULL,
    domain    TEXT NOT NULL,
    score     INTEGER NOT NULL CHECK (score IN (-1, 0, 1)),
    lesson    TEXT NOT NULL,
    source    TEXT NOT NULL DEFAULT 'conversation'
);

CREATE INDEX IF NOT EXISTS idx_outcomes_sender_time ON outcomes (sender_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_outcomes_time ON outcomes (timestamp);

CREATE TABLE IF NOT EXISTS lessons (
    id          TEXT PRIMARY KEY,
    sender_id   TEXT NOT NULL,
    domain      TEXT NOT NULL,
    rule        TEXT NOT NULL,
    occurrences INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(sender_id, domain)
);

CREATE INDEX IF NOT EXISTS idx_lessons_sender ON lessons (sender_id);
