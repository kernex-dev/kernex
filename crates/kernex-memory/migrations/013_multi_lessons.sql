-- Allow multiple lessons per (sender_id, domain, project).
-- Previously UNIQUE(sender_id, domain, project) forced one rule per domain.

CREATE TABLE lessons_v2 (
    id          TEXT PRIMARY KEY,
    sender_id   TEXT NOT NULL,
    domain      TEXT NOT NULL,
    rule        TEXT NOT NULL,
    project     TEXT NOT NULL DEFAULT '',
    occurrences INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO lessons_v2 SELECT * FROM lessons;
DROP TABLE lessons;
ALTER TABLE lessons_v2 RENAME TO lessons;

CREATE INDEX idx_lessons_sender ON lessons (sender_id);
CREATE INDEX idx_lessons_project ON lessons (sender_id, project);
CREATE INDEX idx_lessons_domain ON lessons (sender_id, domain, project);
