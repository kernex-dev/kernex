-- Soft-delete column for the facts table.
--
-- Adds a nullable `deleted_at` timestamp. Active rows have `deleted_at IS
-- NULL`; soft-deleted rows carry the deletion timestamp. All read paths
-- on the facts table filter `WHERE deleted_at IS NULL` so callers see
-- only active rows by default. Hard-delete inherent methods on `Store`
-- still issue raw `DELETE` statements for emergency cleanup; the
-- `MemoryStore` trait surface only exposes soft-delete.

ALTER TABLE facts ADD COLUMN deleted_at TEXT;

-- Partial index over active rows. Speeds up the common `WHERE deleted_at
-- IS NULL` filter. Only active rows are indexed; soft-deleted rows do
-- not pay index maintenance cost on the hot path.
CREATE INDEX IF NOT EXISTS idx_facts_active
    ON facts (sender_id, key)
    WHERE deleted_at IS NULL;
