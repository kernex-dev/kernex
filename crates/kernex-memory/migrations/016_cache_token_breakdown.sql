-- Per-dimension token breakdown for prompt-cache cost telemetry.
-- Anthropic and a few other providers report cache_read / cache_creation
-- separately from regular input/output tokens. The legacy `tokens` column
-- remains the authoritative total; these columns are nullable so older
-- rows (and providers that do not report a breakdown) stay valid.

ALTER TABLE token_usage ADD COLUMN input_tokens          INTEGER;
ALTER TABLE token_usage ADD COLUMN output_tokens         INTEGER;
ALTER TABLE token_usage ADD COLUMN cache_read_tokens     INTEGER;
ALTER TABLE token_usage ADD COLUMN cache_creation_tokens INTEGER;
