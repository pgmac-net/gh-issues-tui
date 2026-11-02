## Context

The load-bearing decision was where the harness gets a terminal. Four options were weighed against the ticket's own wording:

| Option | Multi-session | New deps | Cost |
|---|---|---|---|
| Suspend + exec (`$EDITOR` pattern) | **impossible** | 0 | ~80 lines |
| **Embedded PTY pane** | yes | 3 | ~900 lines |
| tmux-backed | yes | 0 | requires tmux |
| Print command + quit | no | 0 | trivial, weak fit |

The ticket asked for concurrent panes with a switcher, which rules out suspend+exec structurally — it can only ever run one agent, with the TUI gone while it does. tmux-backed would have been cheap but makes the feature unavailable outside tmux and puts the panes outside the TUI's own frame.

`libghostty` — the ticket's own suggestion — has no crates.io binding; it is Ghostty's experimental C embedding API. `tui-term` is the ratatui-native equivalent and is version-compatible with the ratatui already in use.

## Decisions

### The reserved key is `F12`, not a Ctrl chord

Whatever key is reserved becomes unusable inside every harness, permanently. Claude Code alone binds `Ctrl+B/C/D/E/L/O/R/T/V/Z` and `Shift+Tab`; codex and opencode differ again, so any Ctrl chord silently costs each agent a binding, and which one depends on the agent. No agent CLI binds function keys.

One prefix buys the whole vocabulary (`d`/`s`/`k`/`n`/`?`) instead of spending a scarce key per action, and `F12 F12` returns the key itself, so nothing is permanently lost.

### State is split pure/impure

`CLAUDE.md` states `app/` has no I/O. PTY handles, reader threads and child processes are I/O, so:

```
app/harness.rs      HarnessState { sessions: Vec<SessionMeta>, active }   ← pure
tui/harness/mod.rs  HarnessRegistry { HashMap<SessionId, LiveSession> }   ← owned by the event loop
```

The registry is a parameter of the key handlers (`HarnessCtx`), never a field of `App`. Every lifecycle transition — launch, attach, detach, exit, kill, quit-gating — is therefore driven in tests without spawning a process; only one test spawns a real child, to prove the PTY path itself works.

Session ids are monotonic and never reused. A `HarnessDirty`/`HarnessExited` event carrying a dead id can then never be mistaken for a different session that took its slot.

### Output does not travel through the event channel

The reader thread parses bytes straight into its session's `vt100::Parser` behind a mutex, and sends a payload-free `AppEvent::HarnessDirty(id)`. The event loop uses that id only to decide whether a redraw is needed: a detached agent writing at full tilt parses its own output and draws nothing.

Routing the bytes themselves through the channel would have made a chatty agent able to outrun the loop, with the issue list re-rendering per 8 KB chunk.

### Detach ≠ kill, and exit ≠ dismiss

Two separate rules, both about not destroying work:

- Detaching leaves the child running. Only `F12 k` (with a confirmation while it is alive) or quitting ends it.
- A child exiting leaves the session in place with its screen frozen. The closing summary is usually the most valuable thing an agent produces, and auto-removing the pane on exit would discard it at exactly the wrong moment.

The children live on PTYs this process owns, so "detach and let them outlive the TUI" is not achievable without `setsid`/tmux; quitting therefore confirms and terminates rather than pretending otherwise.

### One session per issue, enforced in `launch`

`launch_action` computes what `A` should do, but the picker and `F12 n` reach `HarnessCtx::launch` directly. Putting the rule only in `launch_action` left `F12 n` able to start a second agent on one ticket — caught during end-to-end testing. The check now lives in `launch`, so every entry point obeys it.

### Built-in harnesses are only the verified ones

`claude` and `opencode` ship configured, both checked against their `--help`. `codex`, `copilot` and `pi` are documented as ready-to-paste snippets instead. A shipped default built from a guessed argv fails at spawn time and looks like a bug in this tool rather than a config gap.

## Risks / Trade-offs

- **`F12` on some terminals.** A few terminal emulators and window managers intercept function keys. The chord vocabulary is the only way out of a session, so a user whose terminal eats `F12` is stuck in the pane until the child exits. Mitigation if it bites: make the prefix configurable.
- **vt100 fidelity.** `vt100` implements xterm; agents drawing with unusual sequences may render imperfectly. `TERM=xterm-256color` is pinned for the child so it advertises what the parser actually implements.
- **Sessions are process-lifetime only.** Nothing survives quitting the TUI. Persisting them would mean `setsid` or a tmux backend, which is a different feature.
