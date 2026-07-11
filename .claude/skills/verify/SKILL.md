---
name: verify
description: Build and drive the gh-issues TUI end-to-end to verify changes at the terminal surface, using a pty + pyte screen-capture driver (no tmux required).
---

# Verifying gh-issues

The surface is the terminal TUI. `cargo test`/clippy are CI gates, not
verification — drive the running app and capture screens.

## Build & launch

```sh
cargo build           # binary at target/debug/gh-issues
```

Auth comes from `gh auth token` automatically. Startup org resolution:
`--org` → cwd git `origin` remote (owner + repo filter) → `default_org`
in `~/.config/gh-issues/config.toml`. Fetches hit the real GitHub API
(read-only unless you drive mutations) — occasional GraphQL 503s are
transient; retry.

## Driving it headless

No tmux on this host. Use a pty + [pyte](https://pypi.org/project/pyte/)
screen emulator:

```sh
uv run --with pyte python drive_tui.py <cwd> "<steps literal>"
```

`drive_tui.py`: fork binary on a pty (set TERM/winsize via TIOCSWINSZ),
feed output to `pyte.ByteStream`, print `screen.display` at labelled
checkpoints. Steps are `(bytes_to_send | None, wait_seconds, label | None)`
tuples; end by sending `q`. A working copy exists in past session job dirs
(`~/.claude/jobs/*/tmp/drive_tui.py`) — recreate from this recipe if gone.

Useful key sequences: initial fetch needs ~15s wait; `F` filter menu,
`c` clear-all inside it, Esc closes; `w` org-switch input; `\x15`
(ctrl-u) clears an input; `\r` submits.

## Gotchas

- `pgmac-net/gh-issues-tui` itself has zero issues, so a repo-filtered
  startup from this clone legitimately shows "0 issues" with
  `[filters active]` in the status bar.
- Repos with no issues are omitted from fetch results entirely.
