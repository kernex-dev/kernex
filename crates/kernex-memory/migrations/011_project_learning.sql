-- Project-scoped learning: tag outcomes, lessons, and tasks with a project name.
-- Empty string = general (avoids NULL-in-UNIQUE edge cases).

-- Add project column to outcomes.
ALTER TABLE outcomes ADD COLUMN project TEXT NOT NULL DEFAULT '';

-- Recreate lessons table with project in the unique constraint.
-- SQLite can't ALTER unique constraints, so we must recreate.
CREATE TABLE lessons_new (
    id          TEXT PRIMARY KEY,
    sender_id   TEXT NOT NULL,
    domain      TEXT NOT NULL,
    rule        TEXT NOT NULL,
    project     TEXT NOT NULL DEFAULT '',
    occurrences INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(sender_id, domain, project)
);

INSERT INTO lessons_new (id, sender_id, domain, rule, project, occurrences, created_at, updated_at)
    SELECT id, sender_id, domain, rule, '', occurrences, created_at, updated_at FROM lessons;

DROP TABLE lessons;
ALTER TABLE lessons_new RENAME TO lessons;

-- Add project column to scheduled_tasks.
ALTER TABLE scheduled_tasks ADD COLUMN project TEXT NOT NULL DEFAULT '';

-- Indexes for project-scoped queries.
CREATE INDEX IF NOT EXISTS idx_outcomes_project ON outcomes (sender_id, project, timestamp);
CREATE INDEX IF NOT EXISTS idx_lessons_project ON lessons (sender_id, project);
CREATE INDEX IF NOT EXISTS idx_lessons_sender ON lessons (sender_id);
