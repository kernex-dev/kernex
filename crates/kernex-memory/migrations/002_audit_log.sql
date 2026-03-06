-- Audit log: every interaction through Kernex

CREATE TABLE IF NOT EXISTS audit_log (
    id              TEXT PRIMARY KEY,
    timestamp       TEXT NOT NULL DEFAULT (datetime('now')),
    channel         TEXT NOT NULL,
    sender_id       TEXT NOT NULL,
    sender_name     TEXT,
    input_text      TEXT NOT NULL,
    output_text     TEXT,
    provider_used   TEXT,
    model           TEXT,
    processing_ms   INTEGER,
    status          TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'error', 'denied')),
    denial_reason   TEXT
);

CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_sender ON audit_log(channel, sender_id);
