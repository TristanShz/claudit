-- Tool calls read from transcripts (sessions recorded before `claudit
-- install`, or whose hooks are missing). Both sources upsert the same
-- `tool_calls` row on `tool_use_id`; each keeps its own timing here, and the
-- effective `pre_at_us`, `post_at_us`, `duration_ms` and `success` are
-- resolved from them after every write (hook timing wins once the hook has
-- seen the call complete), so the result never depends on ingest order.

-- Timing as the hooks saw it: PreToolUse / PostToolUse(Failure) receive
-- times, Claude Code's reported execution time, 1 = success.
ALTER TABLE tool_calls ADD COLUMN hook_pre_at_us INTEGER;
ALTER TABLE tool_calls ADD COLUMN hook_post_at_us INTEGER;
ALTER TABLE tool_calls ADD COLUMN hook_duration_ms INTEGER;
ALTER TABLE tool_calls ADD COLUMN hook_success INTEGER;

-- Timing as the transcript saw it: the timestamps of the entries carrying
-- the `tool_use` and its `tool_result`, 1 = the result was not an error.
ALTER TABLE tool_calls ADD COLUMN transcript_pre_at_us INTEGER;
ALTER TABLE tool_calls ADD COLUMN transcript_post_at_us INTEGER;
ALTER TABLE tool_calls ADD COLUMN transcript_success INTEGER;

-- Where the effective timing comes from: `hook`, or `transcript` (then
-- `duration_ms` is post − pre, an estimate that includes any permission
-- prompt).
ALTER TABLE tool_calls ADD COLUMN timing_source TEXT;

-- Every existing row came from hooks.
UPDATE tool_calls
SET hook_pre_at_us   = pre_at_us,
    hook_post_at_us  = post_at_us,
    hook_duration_ms = duration_ms,
    hook_success     = success,
    timing_source    = 'hook';
