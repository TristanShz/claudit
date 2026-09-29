-- #9 Skills and subagents. Both tables are projections of `raw_events`
-- (hooks) plus the transcripts on disk, rebuilt by reingest.

-- One row per skill invocation.
-- - trigger `user`: the user typed `/<skill>` (UserPromptExpansion);
--   invocation_id = `expansion:<prompt_id>` (one expansion per prompt).
-- - trigger `model`: Claude called the Skill tool (PostToolUse);
--   invocation_id = the call's tool_use_id.
CREATE TABLE skill_invocations (
    session_id    TEXT    NOT NULL,
    invocation_id TEXT    NOT NULL,
    prompt_id     TEXT,             -- turn it happened in
    agent_id      TEXT,             -- NULL on the main thread
    skill         TEXT    NOT NULL,
    trigger       TEXT    NOT NULL CHECK (trigger IN ('user', 'model')),
    source        TEXT,             -- `command_source` (user trigger only)
    args          TEXT,
    at_us         INTEGER NOT NULL, -- receive time
    PRIMARY KEY (session_id, invocation_id)
);
CREATE INDEX skill_invocations_by_time ON skill_invocations (at_us);

-- One row per subagent run, merged from SubagentStart/SubagentStop, the
-- parent's Agent/Task tool response and the subagent's transcript.
-- agent_id is the bare id (`a1f3…`, as in `subagents/agent-<id>.jsonl`).
CREATE TABLE subagent_runs (
    agent_id             TEXT PRIMARY KEY,
    session_id           TEXT    NOT NULL,
    prompt_id            TEXT,             -- parent turn
    agent_type           TEXT,
    parent_tool_use_id   TEXT,             -- the Agent/Task call that ran it
    model                TEXT,             -- `resolvedModel` of the Agent response
    started_at_us        INTEGER,          -- earliest known start
    stopped_at_us        INTEGER,          -- latest known stop
    total_duration_ms    INTEGER,          -- `totalDurationMs`
    total_tool_use_count INTEGER           -- `totalToolUseCount`
);
CREATE INDEX subagent_runs_by_session ON subagent_runs (session_id);
CREATE INDEX tool_calls_by_agent ON tool_calls (agent_id);
CREATE INDEX api_messages_by_agent ON api_messages (agent_id);
CREATE INDEX api_messages_by_skill ON api_messages (skill);
