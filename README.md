# claudit

Local analytics for your [Claude Code](https://code.claude.com) sessions:
where the time goes, which tools, commands, skills and subagents Claude uses,
and what it all costs. Everything stays on your machine.

## Quick start

macOS (Apple silicon or Intel):

```sh
VERSION=v0.5.0
TARGET="$([ "$(uname -m)" = arm64 ] && echo aarch64 || echo x86_64)-apple-darwin"
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/TristanShz/claudit/releases/download/$VERSION/claudit-$VERSION-$TARGET.tar.gz" \
  | tar -xz --strip-components=1 -C ~/.local/bin "claudit-$VERSION-$TARGET/claudit"

claudit install   # record your Claude Code sessions
claudit serve     # open http://127.0.0.1:8421
```

Make sure `~/.local/bin` is on your `PATH`. With Rust:
`cargo install --locked --git https://github.com/TristanShz/claudit`.

That's it: new sessions are recorded automatically, and your existing
history is imported on the first run. To upgrade, run `claudit update`; to
stop, run `claudit uninstall`.

![Overview page](docs/screenshots/overview.png)

## What you get

- **Where the time goes**: model, tools, waiting on you, subagents.
- **Bash commands**: the most used and the slowest (`yarn test`,
  `cargo build`…), per project and per session.
- **Activities**: time spent running tests, building, using git, editing…
- **Tools, skills, subagents and models**, with tokens and an
  API-equivalent cost.
- **Session detail**: every turn, and a timeline of each tool call and
  subagent.

![Session detail page](docs/screenshots/session.png)

## Good to know

- **Private by design**: no account, no telemetry, no network (except
  `claudit update`, when you run it). The dashboard
  only listens on `127.0.0.1`, files are readable by you only, and secrets
  (API keys, tokens, passwords) are redacted before anything is stored.
- **Your prompts and tool inputs are stored; tool outputs and Claude's
  answers never are.**
- **Time is measured from the moment you install.** Sessions imported from
  before that show tokens, cost and tool calls, but no time breakdown.
- The cost is an estimate at API list prices, not what your subscription
  costs.
- `claudit install` backs up `~/.claude/settings.json` first and never
  touches your own hooks.

## Learn more

- [Guide](docs/GUIDE.md): commands, what is captured, custom activity
  rules, querying the SQLite archive, limitations.
- [ARCHITECTURE.md](ARCHITECTURE.md): how it works inside.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [CHANGELOG.md](CHANGELOG.md).

## License

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)).
