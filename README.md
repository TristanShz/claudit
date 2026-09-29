# claudit

Local analytics for [Claude Code](https://code.claude.com) sessions.

claudit records every Claude Code session on your machine into a permanent
SQLite archive, through async hooks that never slow your sessions down, and
serves a local dashboard showing where the time goes, which tools, skills and
subagents are used, and what the tokens would cost at API prices.

> **Status: early development.** Today claudit captures `PostToolUse` hook
> events and shows a tool ranking. `claudit install` is not implemented yet.

## Try it

```sh
cargo build --release

# Wire one hook by hand in ~/.claude/settings.json:
# {"hooks": {"PostToolUse": [{"hooks": [
#   {"type": "command", "command": "/path/to/claudit hook", "async": true}
# ]}]}}

claudit ingest          # load recorded events into ~/.claudit/claudit.db
claudit serve           # dashboard on http://127.0.0.1:8421 (--port to change)
```

`CLAUDIT_HOME` (default `~/.claudit`) and `CLAUDE_CONFIG_DIR` (default
`~/.claude`) are respected. The dashboard listens on localhost only and
works offline: all assets are embedded in the binary.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
