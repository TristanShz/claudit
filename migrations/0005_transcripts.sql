-- #5 Transcript ingestion: sessions, turns and API messages derived from
-- Claude Code's transcripts (main sessions and subagents).
-- Timestamps are INTEGER microseconds since the Unix epoch (suffix `_us`).

-- One row per Claude Code session, from its main-thread transcript entries.
CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    cwd         TEXT,             -- first working directory seen
    git_branch  TEXT,             -- branch at the latest entry
    version     TEXT,             -- Claude Code version at the latest entry
    source      TEXT,             -- startup/resume/clear/compact (SessionStart hook)
    first_at_us INTEGER,          -- earliest entry (NULL until a transcript is seen)
    last_at_us  INTEGER           -- latest entry
);
CREATE INDEX sessions_by_first ON sessions (first_at_us);

-- One row per turn (user prompt), keyed like the hooks key it
-- (`session_id` + `prompt_id`), so transcripts and hooks both upsert it.
-- Transcript and hook timings live in separate columns; readers prefer the
-- hook ones when present.
CREATE TABLE turns (
    session_id      TEXT NOT NULL,
    prompt_id       TEXT NOT NULL,
    prompt_text     TEXT,             -- first non-meta prompt text of the turn
    permission_mode TEXT,
    effort          TEXT,
    start_at_us     INTEGER,          -- earliest main-thread transcript entry
    end_at_us       INTEGER,          -- latest main-thread user/assistant entry
    submit_at_us    INTEGER,          -- UserPromptSubmit receive time (#8)
    stop_at_us      INTEGER,          -- Stop receive time (#8)
    PRIMARY KEY (session_id, prompt_id)
);
CREATE INDEX turns_by_start ON turns (start_at_us);

-- One row per API response. A response streamed over several transcript
-- entries shares `message_id` and repeats its usage (output tokens growing),
-- so tokens are upserted with MAX: each response counts once.
CREATE TABLE api_messages (
    message_id         TEXT PRIMARY KEY,
    session_id         TEXT    NOT NULL,
    prompt_id          TEXT,             -- turn it belongs to
    agent_id           TEXT,             -- NULL on the main thread
    agent_type         TEXT,             -- subagent type (`attributionAgent`)
    model              TEXT    NOT NULL,
    at_us              INTEGER NOT NULL, -- first entry's timestamp
    input_tokens       INTEGER NOT NULL,
    output_tokens      INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    cache_read_tokens  INTEGER NOT NULL,
    skill              TEXT              -- `attributionSkill`
);
CREATE INDEX api_messages_by_session ON api_messages (session_id, prompt_id);
CREATE INDEX api_messages_by_time ON api_messages (at_us);

-- Every transcript entry uuid already ingested (deduplication: resumed
-- sessions repeat earlier entries in a new file), with the turn it belongs
-- to, so entries without a `promptId` inherit their parent's.
CREATE TABLE transcript_entries (
    uuid       TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    prompt_id  TEXT
);
