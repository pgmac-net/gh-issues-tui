## Why

Issue [#23](https://github.com/pgmac-net/gh-issues-tui/issues/23). Reading an issue and deciding to work on it currently means leaving the TUI, finding the repo's clone, and retyping the reference into a coding agent. The ticket asks for that handoff from inside the TUI, and is explicit that it must not be Claude-only — codex, copilot, opencode and pi have to be equally first-class.

The ticket floated `libghostty` for the embedded terminal. That was ruled out on facts: libghostty is Ghostty's experimental C embedding API, Zig-built, with no crates.io binding. The ratatui-native equivalent is `tui-term` + `portable-pty` + `vt100`, and `tui-term` 0.3.4 targets `ratatui-core` 0.1 / `ratatui-widgets` 0.3 — the ratatui 0.30 family already in `Cargo.toml`.

Grilling turned three questions into the shape of the feature:

- *What happens when focus is lost to the pane?* Agent CLIs claim nearly every Ctrl chord, so exactly one key can be reserved and it must not be one of theirs.
- *Can the pane only close when the command exits?* Yes — and detach must therefore be distinct from kill.
- *Can several run at once with a switcher?* Yes, but only if the session registry exists from the start; retrofitting one-session code into multi-session later is the expensive path.

## What Changes

A harness session is a coding agent running on its own PTY, keyed by issue reference, rendered full-frame inside the TUI.

- `A` on an issue starts `default_harness` for it, in that repo's clone. A second press attaches to the running session rather than starting a second agent on one ticket.
- `Z` opens a picker over every session; the status bar carries a persistent `n running, m exited` segment.
- In a session, **every key goes to the child** except `F12`, a tmux-style prefix: `F12 d` detach, `s` switch, `k` kill, `n` new, `?` help, `F12 F12` sends a literal `F12`.
- A child that exits leaves its session in place, screen frozen and scrollable, so the agent's closing summary survives. Only `F12 k` (or a relaunch) removes it.
- `q` with live sessions opens a confirmation naming them before terminating.
- `[harnesses.<name>]` config tables define harnesses as **argv arrays**, with `{owner}`, `{repo}`, `{number}`, `{ref}` and `{url}` placeholders. `claude` and `opencode` ship as built-ins.
- `workspace_roots` resolves a repo's clone; the cwd's own repo wins when it matches.

- **BREAKING**: none. `A`, `Z` and `F12` were unbound; every existing key keeps its meaning.

## Capabilities

### New Capabilities

- `harness-sessions`: launching coding agents on PTYs from the issue list, and the lifecycle rules governing them.

### Modified Capabilities

(none)

## Impact

- **Affected code**: new `src/tui/harness/` (registry, spawn, key encoder) and `src/tui/app/harness.rs` (pure state); `config.rs`, `layout.rs`, the event loop, the normal-mode keys, and three new UI dispatch arms.
- **New dependencies**: `tui-term` 0.3, `portable-pty` 0.9, `vt100` 0.16.
- **Security**: harness commands are argv arrays, never shell strings. Issue titles, bodies and URLs are attacker-controlled in a public org; expanding a placeholder through `sh -c` would be a live injection hole. Every placeholder expands into exactly one argv slot, pinned by `expansion_never_splits_an_argument`.
- **Risk to watch**: the reserved key. Whatever is reserved is unusable inside *every* harness, forever. `F12` was chosen because no agent CLI binds function keys — a Ctrl chord would have silently cost each of them a binding.
- **Deliberately not done**: cloning a repo on demand when no clone is found. The error names every path it tried; cloning is a separate decision with its own failure modes.
