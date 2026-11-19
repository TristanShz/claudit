-- #27 Dashboard cold load. Every report over `tool_calls` scanned a table
-- made mostly of input JSON (two thirds of its bytes) to read a few small
-- columns. The JSON and the error text move to `tool_call_payloads`, read
-- only where a call is shown; `tool_calls` keeps what the aggregates read,
-- plus the Bash command line they classify.

-- A call's bulky text, one row per call that has any. Projection of
-- `raw_events` and transcripts (rebuilt by reingest).
CREATE TABLE tool_call_payloads (
    tool_use_id TEXT PRIMARY KEY, -- as in `tool_calls`
    tool_input  TEXT,             -- redacted input JSON
    error       TEXT              -- error text of a hook-reported failure
);
INSERT INTO tool_call_payloads (tool_use_id, tool_input, error)
SELECT tool_use_id, tool_input, error
FROM tool_calls
WHERE tool_input IS NOT NULL OR error IS NOT NULL
ORDER BY rowid;

-- `tool_calls` without `tool_input` and `error`, rebuilt rather than
-- altered so its pages are packed again.
CREATE TABLE tool_calls_slim (
    tool_use_id           TEXT PRIMARY KEY,
    session_id            TEXT    NOT NULL,
    prompt_id             TEXT,    -- turn the call belongs to
    agent_id              TEXT,    -- NULL on the main thread
    tool_name             TEXT    NOT NULL,
    mcp_server            TEXT,
    bash_command          TEXT,    -- leading command of a Bash call
    command_line          TEXT,    -- whole command line of a Bash call
    cwd                   TEXT,
    pre_at_us             INTEGER, -- effective timing, resolved from the
    post_at_us            INTEGER, -- hook_* and transcript_* columns
    duration_ms           INTEGER,
    success               INTEGER,
    hook_pre_at_us        INTEGER, -- PreToolUse receive time
    hook_post_at_us       INTEGER, -- PostToolUse(Failure) receive time
    hook_duration_ms      INTEGER, -- execution time reported by Claude Code
    hook_success          INTEGER, -- 1 = PostToolUse, 0 = PostToolUseFailure
    transcript_pre_at_us  INTEGER, -- timestamp of the `tool_use` entry
    transcript_post_at_us INTEGER, -- timestamp of the `tool_result` entry
    transcript_success    INTEGER, -- 1 = the result was not an error
    timing_source         TEXT     -- `hook` or `transcript`
);
INSERT INTO tool_calls_slim (
    tool_use_id, session_id, prompt_id, agent_id, tool_name, mcp_server,
    bash_command, command_line, cwd, pre_at_us, post_at_us, duration_ms,
    success, hook_pre_at_us, hook_post_at_us, hook_duration_ms, hook_success,
    transcript_pre_at_us, transcript_post_at_us, transcript_success,
    timing_source
)
SELECT tool_use_id, session_id, prompt_id, agent_id, tool_name, mcp_server,
       bash_command,
       CASE WHEN tool_name = 'Bash' AND json_valid(tool_input)
                 AND json_type(tool_input, '$.command') = 'text'
            THEN json_extract(tool_input, '$.command') END,
       cwd, pre_at_us, post_at_us, duration_ms,
       success, hook_pre_at_us, hook_post_at_us, hook_duration_ms, hook_success,
       transcript_pre_at_us, transcript_post_at_us, transcript_success,
       timing_source
FROM tool_calls
ORDER BY rowid;

DROP TABLE tool_calls;
ALTER TABLE tool_calls_slim RENAME TO tool_calls;

-- A session's calls and their count (`post_at_us` makes the count of its
-- completed calls index-only).
CREATE INDEX tool_calls_by_session ON tool_calls (session_id, post_at_us);
-- Completed calls by time, covering the tool ranking.
CREATE INDEX tool_calls_by_post
    ON tool_calls (post_at_us, tool_name, success, hook_duration_ms, session_id);
-- A turn's calls, covering the time decomposition's per-turn lookup.
CREATE INDEX tool_calls_by_prompt
    ON tool_calls (session_id, prompt_id, agent_id, hook_post_at_us,
                   hook_pre_at_us, hook_duration_ms, tool_name);
-- A subagent's calls and their count.
CREATE INDEX tool_calls_by_agent ON tool_calls (agent_id, post_at_us);
