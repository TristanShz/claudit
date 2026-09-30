-- #2 Tracer bullet: raw hook events, per-file ingest progress, tool calls.
-- Timestamps are INTEGER microseconds since the Unix epoch (suffix `_us`).

-- Append-only archive of every hook payload: the replay source for
-- `claudit reingest`. Normalized tables are projections of these rows.
CREATE TABLE raw_events (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT    NOT NULL,
    hook_event_name TEXT    NOT NULL,
    received_at_us  INTEGER NOT NULL,
    payload         TEXT    NOT NULL -- JSON
);
CREATE INDEX raw_events_by_session ON raw_events (session_id, received_at_us);

-- Byte offset reached in each input file (spool or transcript), keyed by
-- file identity so a replaced file starts over.
CREATE TABLE ingest_offsets (
    path          TEXT    NOT NULL,
    inode         INTEGER NOT NULL,
    byte_offset   INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    PRIMARY KEY (path, inode)
);

-- One row per tool call, deduplicated on tool_use_id. Columns filled by
-- later tickets (PreToolUse pairing, failures, Bash/MCP breakdowns,
-- subagent attribution) are created here, nullable, to avoid churn.
CREATE TABLE tool_calls (
    tool_use_id  TEXT PRIMARY KEY,
    session_id   TEXT    NOT NULL,
    prompt_id    TEXT,             -- turn the call belongs to
    agent_id     TEXT,             -- NULL on the main thread
    tool_name    TEXT    NOT NULL,
    mcp_server   TEXT,
    bash_command TEXT,             -- leading command of a Bash call
    tool_input   TEXT,             -- JSON
    cwd          TEXT,
    pre_at_us    INTEGER,          -- PreToolUse receive time
    post_at_us   INTEGER,          -- PostToolUse(Failure) receive time
    duration_ms  INTEGER,          -- execution time reported by Claude Code
    success      INTEGER,          -- 1 = PostToolUse, 0 = PostToolUseFailure
    error        TEXT
);
CREATE INDEX tool_calls_by_session ON tool_calls (session_id);
CREATE INDEX tool_calls_by_post ON tool_calls (post_at_us);
