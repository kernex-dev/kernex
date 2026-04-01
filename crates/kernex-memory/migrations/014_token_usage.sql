-- Persistent token usage and estimated cost tracking per sender/session.

CREATE TABLE token_usage (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    sender_id  TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    model      TEXT    NOT NULL,
    tokens     INTEGER NOT NULL DEFAULT 0,
    cost_usd   REAL    NOT NULL DEFAULT 0.0,
    timestamp  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_token_usage_session ON token_usage (session_id);
CREATE INDEX idx_token_usage_sender  ON token_usage (sender_id);
