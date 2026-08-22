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

`F12 ?` and `?` open the same `Mode::Help` popup but render different tables, chosen on
`harness.active.is_some()` — the flag the dismiss path already uses to decide where help
returns to, and which `detach()` clears. The tables share no keys: a session forwards `n`
and `/` to the child, so listing them as app keys inside one would be wrong, and before
#132 the `F12` chords appeared nowhere in the app at all.

## Session chrome (#132)

A session is drawn full-frame and the agent owns everything between the two chrome rows:

```
█ gh-issues-tui █ pgmac-net/gh-issues-tui#132 · Send to Claude/harness · claude · ● running
  (the child's screen, from its vt100 parser)
 F12 d detach · s switch · k kill · n new · F12 F12 literal · ? help
```

**The rows come out of the child's PTY.** `layout::harness_areas().pane` is what reaches
`PtySize`, both at spawn and through `resize_sessions`, so each row of chrome is a row the
agent does not get. That is why the identity row is one row and not a border: a border
costs two rows *and* two columns, narrowing every agent TUI permanently.

The rows are shed as the terminal shrinks, header first — the keys are the half a user
cannot recover by looking, while the header only names the session they just opened:

| Frame height | header | pane | keys |
|---|---|---|---|
| `>= 3` | 1 | rest | 1 |
| `2` | 0 | 1 | 1 |
| `1` | 0 | 1 | 0 |

The pane never reaches zero rows: a `PtySize` with zero rows is rejected by the OS.

Under width pressure the identity row gives up its parts in order — the title elides
first, then the owner drops off the reference (`…/gh-issues-tui#132`). The brand badge and
the running state never elide.

### Terminal title

`tui/title.rs` sets `OSC 2` to `gh-issues-tui · <issue>` while a session is attached, and
back to `gh-issues-tui` on detach, so provenance survives the TUI not being the focused
pane. It is driven off the drawn state once per loop iteration rather than hooked into
attach/detach/kill/exit separately, so it cannot drift out of step with what is on screen;
a dedup on the last-written value makes the repeat calls free.

Restoration is covered twice: `Drop` on the guard for normal exits and unwinding panics,
and `title::restore()` in `main`'s panic hook, which runs *before* unwinding and so may be
the only one reached.

Titles are sanitised before they are written. Repo names and issue titles come from the
API, and a `BEL` in one would terminate the `OSC` early, leaving the rest to reach the
terminal as its own input; an `ESC` could open a fresh sequence. Every control character
is dropped and the result capped at 128 chars.

Two deliberate non-goals. There is no portable way to read the previous title back (the
`21t` query is widely disabled as a security measure), so restore writes a fixed name
rather than whatever was there before. And a child that emits its own `OSC 2` will win —
the title is a secondary signal, which is exactly why the identity row exists.

### Provenance environment

`set_provenance_env` stamps every child with `GH_ISSUES_TUI` (marker and version),
`_HARNESS`, `_OWNER`, `_REPO`, `_NUMBER`, `_ISSUE`, `_URL` and `_TITLE`. The README lists
the values and a worked Claude Code `statusLine` recipe.

The agent already learns its ticket from argv — the builtin `claude` command passes
`/pgmac-workflows:pickup-ticket {ref}` as the prompt. What argv cannot tell anything is
the *launcher*, and that is what hooks, statuslines and scripts need, including ones the
agent spawns itself. Scraping the parent's argv does not generalise: the `opencode`
default carries a URL with no `#N` in it.

These go through `CommandBuilder::env`, so the values are never parsed by a shell — the
same property the argv array gives placeholder expansion (see [Security](#security)).

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
