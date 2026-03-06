-- Memory enhancement: conversation boundaries, summaries, facts scoping

ALTER TABLE conversations ADD COLUMN summary TEXT;
ALTER TABLE conversations ADD COLUMN last_activity TEXT NOT NULL DEFAULT (datetime('now'));
ALTER TABLE conversations ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
CREATE INDEX IF NOT EXISTS idx_conversations_status ON conversations(status, last_activity);

-- Recreate facts table with sender_id scoping (currently unused/empty)
DROP TABLE IF EXISTS facts;
CREATE TABLE facts (
    id                TEXT PRIMARY KEY,
    sender_id         TEXT NOT NULL,
    key               TEXT NOT NULL,
    value             TEXT NOT NULL,
    source_message_id TEXT REFERENCES messages(id),
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(sender_id, key)
);
