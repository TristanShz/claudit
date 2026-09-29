# Hook payload fixtures

One JSON file per hook payload, named `<event>_<variant>.json` in snake case
(e.g. `post_tool_use_bash.json`). Tests replay them through the same entry
point `claudit hook` uses (`TestEnv::hook_fixture`), optionally overriding
fields (`TestEnv::hook_fixture_with`) to vary ids, tools or durations.

Shapes follow Claude Code 2.1.284 (https://code.claude.com/docs/en/hooks),
anonymized: user `alice`, project `/Users/alice/code/acme-api`, made-up ids.
When upstream shapes change, add new fixtures next to these (suffix the
version, e.g. `post_tool_use_bash.v2_3.json`) instead of replacing them.
