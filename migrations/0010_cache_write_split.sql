-- #10: split cache writes by TTL, since 1-hour writes cost more than
-- 5-minute ones. `cache_write_tokens` stays the total of both; this column
-- holds the part of it written with a 1-hour TTL
-- (`usage.cache_creation.ephemeral_1h_input_tokens`). Transcripts without the
-- split give 0, i.e. every write is priced as a 5-minute write. Upserted with
-- MAX like the other token columns.
ALTER TABLE api_messages ADD COLUMN cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0;
