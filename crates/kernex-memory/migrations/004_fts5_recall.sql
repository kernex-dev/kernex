-- FTS5 virtual table for cross-conversation recall (content-sync: index only, reads from messages)
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content='messages',
    content_rowid='rowid'
);

-- Backfill existing user messages
INSERT INTO messages_fts(rowid, content)
    SELECT rowid, content FROM messages WHERE role = 'user';

-- Auto-sync triggers (user messages only)
CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages WHEN NEW.role = 'user'
BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (NEW.rowid, NEW.content);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages WHEN OLD.role = 'user'
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', OLD.rowid, OLD.content);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages WHEN NEW.role = 'user'
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', OLD.rowid, OLD.content);
    INSERT INTO messages_fts(rowid, content) VALUES (NEW.rowid, NEW.content);
END;
