CREATE TABLE IF NOT EXISTS user_aliases (
    alias_sender_id     TEXT PRIMARY KEY,
    canonical_sender_id TEXT NOT NULL
);
