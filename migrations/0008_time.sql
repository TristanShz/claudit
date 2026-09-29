-- #8 Time decomposition: the hook events that time a turn. Turn submit/stop
-- times fill `turns.submit_at_us` / `turns.stop_at_us` and PreToolUse fills
-- `tool_calls.pre_at_us` (all created by earlier migrations); this adds the
-- permission prompts and notifications, and the session end.
-- Every table here is a projection of `raw_events` (rebuilt by reingest).

-- One row per PermissionRequest hook: Claude Code asked the user to allow a
-- tool call. The payload has no `tool_use_id`, so the natural key is the
-- event itself (session, receive time, tool).
CREATE TABLE permission_requests (
    session_id  TEXT    NOT NULL,
    prompt_id   TEXT,
    agent_id    TEXT,             -- NULL on the main thread
    tool_name   TEXT    NOT NULL,
    tool_input  TEXT,             -- JSON
    cwd         TEXT,
    at_us       INTEGER NOT NULL, -- receive time
    PRIMARY KEY (session_id, at_us, tool_name)
);
CREATE INDEX permission_requests_by_time ON permission_requests (at_us);

-- One row per Notification hook (permission prompts, idle prompts, …).
CREATE TABLE notifications (
    session_id        TEXT    NOT NULL,
    prompt_id         TEXT,
    notification_type TEXT    NOT NULL,
    message           TEXT,
    cwd               TEXT,
    at_us             INTEGER NOT NULL, -- receive time
    PRIMARY KEY (session_id, at_us, notification_type)
);
CREATE INDEX notifications_by_time ON notifications (at_us);

-- SessionEnd: when and why the session ended.
ALTER TABLE sessions ADD COLUMN ended_at_us INTEGER;
ALTER TABLE sessions ADD COLUMN end_reason TEXT;

CREATE INDEX turns_by_submit ON turns (submit_at_us);
CREATE INDEX tool_calls_by_prompt ON tool_calls (session_id, prompt_id);
