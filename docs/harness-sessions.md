# Harness sessions (#23)

Send an issue straight to a coding agent, without leaving the TUI.

Press `A` on an issue and the configured harness starts in that repo's clone, already
primed with the ticket. Detach with `F12 d`, keep browsing, `Z` back into it. Several
sessions run at once, each on its own PTY, each keyed to the issue it was launched for.

## Keys

From the issue list:

| Key | Effect |
|---|---|
| `A` | launch `default_harness` for the selected issue; attaches instead if that issue already has a live session; asks first if its previous session exited; opens the harness picker when no default is set |
| `Z` | session picker — every session with its issue, harness and state |
| `q` | with sessions still running, confirms and names them before terminating |

Inside a session **every key goes to the child** — arrows, `Esc`, `Ctrl+C` and
`Shift+Tab` included, because agent CLIs bind them. `F12` is the one key the TUI keeps:

| Chord | Effect |
|---|---|
| `F12 d` | detach — back to the list, child keeps running |
| `F12 s` | switch to another session |
| `F12 k` | kill (confirmation while the child is alive) |
| `F12 n` | start another harness for the current issue |
| `F12 ?` | help |
| `F12 F12` | send a literal `F12` to the child |

Once a child has exited its session stays put, screen frozen, so its closing summary is
still readable: `k`/`j` and `PageUp`/`PageDown` scroll, `G` jumps to the end, `q` leaves
it in place, `x` dismisses it.

The bottom status line carries `n running, m exited (Z)` whenever any session exists —
without it a detached agent would be working with nothing on screen to say so.

## Configuration

`~/.config/gh-issues/config.toml`:

```toml
default_harness = "claude"
workspace_roots = ["~/pgmac", "~/projects"]

[harnesses.claude]
command = ["claude", "/pgmac-workflows:pickup-ticket {ref}"]
```

`command` is an **argv array, never a shell string**. Placeholders expand into
individual argv slots:

| Placeholder | Expands to |
|---|---|
| `{owner}` | `pgmac-net` |
| `{repo}` | `gh-issues-tui` |
| `{number}` | `23` |
| `{ref}` | `pgmac-net/gh-issues-tui#23` |
| `{url}` | the issue's URL |

`{ref}` is always canonical, deliberately independent of `copy_format` — that template is
a clipboard preference and need not identify an issue at all.

`claude` and `opencode` are built in; defining `[harnesses.claude]` overrides the built-in
rather than sitting alongside it, and defining a new harness does not remove the others.
Each harness may override `workspace_roots` for itself.

### Harnesses that are not built in

Their argument forms were not verified, so they are documented rather than shipped —
a default built from a guessed argv fails at spawn time and looks like a bug in this tool:

```toml
[harnesses.codex]
command = ["codex", "work on {url}"]

[harnesses.copilot]
command = ["copilot", "-p", "work on {ref}"]

[harnesses.pi]
command = ["pi", "{ref}"]
```

Check each CLI's own `--help` before relying on these.

### Where a harness runs

1. The current directory's repo, when its `origin` is the issue's repo — so launching from
   inside a clone always does the obvious thing.
2. Otherwise the first `<root>/<repo>` that exists, across `workspace_roots` in order.
3. Otherwise nothing is launched, and the message names every path it tried.

Cloning on demand is deliberately not done: it is a separate decision with its own
failure modes.

## How it works

Three crates: `portable-pty` runs the child on a real PTY, `vt100` parses its output into
a screen, `tui-term` draws that screen into a ratatui pane.

State is split in two, because `app/` has no I/O (see `CLAUDE.md`):

```
app/harness.rs      HarnessState { sessions, active }   pure metadata
tui/harness/mod.rs  HarnessRegistry { id -> LiveSession }   PTYs, threads, parsers
```

The registry is owned by the event loop and reaches the key handlers as `HarnessCtx`,
never as a field of `App`. Every lifecycle transition is therefore unit-testable without
spawning anything; exactly one test (`spawns_a_child_reads_its_output_and_reports_the_exit_code`)
starts a real process, to prove the PTY path itself works.

Each session gets two threads: one draining the PTY into its parser, one waiting on the
child. **Output never crosses the event channel** — the reader parses into the parser
behind a mutex and sends a payload-free `AppEvent::HarnessDirty(id)`. The loop uses that
id only to decide whether to redraw, so a detached agent writing at full tilt draws
nothing and cannot stall the interface.

Session ids are monotonic and never reused, so an event carrying a dead id can never
address whichever session took its slot.

### Why `F12`

Whatever key is reserved is unusable inside *every* harness, forever. Claude Code alone
binds `Ctrl+B/C/D/E/L/O/R/T/V/Z` and `Shift+Tab`; codex and opencode differ again, so any
Ctrl chord silently costs each agent one binding. No agent CLI binds function keys. One
prefix then buys the whole vocabulary rather than spending a scarce key per action, and
`F12 F12` gives the key back.

The trade-off: a terminal or window manager that swallows `F12` leaves you stuck in a
session until its child exits. If that bites, the prefix should become configurable.

### Security

Harness commands are argv arrays and are never passed to a shell. Issue titles, bodies and
URLs are attacker-controlled in a public org; expanding a placeholder through `sh -c` would
be a live injection hole. Each placeholder expands into exactly one argument, whatever it
contains — pinned by `expansion_never_splits_an_argument` in `tui/harness/mod.rs`.
