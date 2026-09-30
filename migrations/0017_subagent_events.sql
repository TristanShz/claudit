-- #17 Subagent run time from its hooks. A run can stop and be resumed (a
-- SendMessage to a background agent), so SubagentStart / SubagentStop are
-- kept one row per event rather than merged into `subagent_runs`: reports
-- pair each start with the next stop, and a run's active time is the sum of
-- those spans. Projection of `raw_events` (rebuilt by reingest).
CREATE TABLE subagent_events (
    agent_id   TEXT    NOT NULL, -- bare id, as in `subagent_runs`
    session_id TEXT    NOT NULL,
    prompt_id  TEXT,             -- as the payload carries it
    event      TEXT    NOT NULL CHECK (event IN ('start', 'stop')),
    at_us      INTEGER NOT NULL, -- receive time
    PRIMARY KEY (agent_id, at_us, event)
);
CREATE INDEX subagent_events_by_session ON subagent_events (session_id);
