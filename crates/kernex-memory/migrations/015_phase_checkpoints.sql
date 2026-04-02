CREATE TABLE IF NOT EXISTS phase_checkpoints (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL,
    topology_name TEXT NOT NULL,
    phase_name    TEXT NOT NULL,
    sender_id     TEXT NOT NULL,
    project       TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'pending'
                      CHECK (status IN ('pending', 'in_progress', 'completed', 'failed')),
    output        TEXT,
    error_message TEXT,
    attempt       INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (run_id, phase_name)
);

CREATE INDEX IF NOT EXISTS idx_phase_checkpoints_run
    ON phase_checkpoints (run_id);

CREATE INDEX IF NOT EXISTS idx_phase_checkpoints_sender
    ON phase_checkpoints (sender_id, project);
