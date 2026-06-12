-- Task execution history + claim support for scheduled tasks.
--
-- `task_runs` records what every scheduled-task execution produced so
-- results stop being fire-and-forget: one row per run, keyed by the
-- owning task id, carrying the outcome status, the response text (or
-- error), and the billed token count when known.
--
-- `scheduled_tasks.claimed_at` supports atomic claim-on-read: a poller
-- claims due tasks by flipping status 'pending' -> 'claimed' in a single
-- UPDATE ... RETURNING, so any number of concurrent pollers can run
-- against the same store and each due task fires exactly once. Stale
-- claims (a claimer that died mid-run) become reclaimable after a
-- timeout window enforced in the claim query.

ALTER TABLE scheduled_tasks ADD COLUMN claimed_at TEXT;

CREATE TABLE IF NOT EXISTS task_runs (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    finished_at  TEXT NOT NULL DEFAULT (datetime('now')),
    status       TEXT NOT NULL CHECK (status IN ('completed', 'failed')),
    result       TEXT,
    error        TEXT,
    tokens_used  INTEGER
);

CREATE INDEX IF NOT EXISTS idx_task_runs_task
    ON task_runs (task_id, started_at DESC);
