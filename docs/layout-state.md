# Layout between sessions (#236)

When you quit, the app remembers where you were; the next start resumes there. Terms: [CONTEXT.md](../CONTEXT.md#layout-236).

## What is saved

| Saved | Not saved |
|---|---|
| Filters (text, repo, assignee, author, priority, status, the six date bounds, hide-empty), the state filter (open/closed/all), sort key and direction | Which org is browsed: startup still decides it, and the layout is looked up *for* it |
| Collapsed repo groups, the repos already seen, collapsed sub-issue parents | Harness sessions: they reattach on their own |
| The selected issue (by id) or repo header (by name), and the list position | Popups and half-typed input |
| The detail pane: open or closed, focus, the comments or runs tab, folded comments | The fetched data (always refetched) and semantic-search hits (re-asked once data lands) |

"Repos already seen" is saved so a repo you expanded stays expanded: otherwise `default_collapsed` would fold it again as if it were new.

## One layout per view

A *view* is the org browsed plus the clone the app was started in, if any.

| Started | File |
|---|---|
| anywhere, browsing `pgmac-net` | `pgmac-net.json` |
| inside the `gh-issues-tui` clone | `pgmac-net@gh-issues-tui.json` |

- The whole-org view and each clone keep their own place. Copies on different views never touch each other's file, so there is no locking.
- Two copies on the **same** view: the last to write wins. Every write is atomic (a per-process temp file, then a rename), so the file is never half-written and two copies can't trip over one temp file.
- The clone names the view only while the org is the one it belongs to: after `w` to another org, that org's own layout is used.
- Names are reduced to `A-Z a-z 0-9 - _ .`, anything else becomes `_`, so a hostile remote can't write outside the directory.

### Precedence

What you pass at startup beats what was saved: `--org`, the repo filter detected from the clone, and `--all`. Everything else comes from the layout.

### Switching org (`w`)

The old org's layout is written as it was left, then the new org's is resumed. Switching back and forth keeps each where you left it. The new org's fetch is already in flight when its layout is applied, so a restored state filter that needs closed issues triggers the usual automatic widen (#230).

## Where and what format

- `~/.local/state/gh-issues/layout/` (XDG state; `$XDG_STATE_HOME` is honoured).
- macOS and Windows have no state directory, so the local-data one stands in: `~/Library/Application Support/gh-issues/layout/`, `%LOCALAPPDATA%\gh-issues\layout\`.
- JSON, like the label-rank cache: serde already does it, and it is machine-written (TOML is for the hand-edited config). It carries a `version` and the backend's name. A missing, corrupt, older-version or other-backend file is ignored, never an error, because issue ids differ between backends.

## When it is saved

- On quit, and on org switch.
- About two seconds after any change to the layout (selecting a row counts), so closing the terminal window or a crash loses at most that much. Checked live: after `kill -9` the file was there and the next start resumed from it.
- Never from the panic hook, which must stay minimal.
- Saving is best-effort: a failed write is never worth interrupting the app for.

## Opting out

- `restore_layout = false` in the config: never restore, never save.
- `--fresh`: start this session clean, but still save on exit, so the next start resumes from it.

## How resuming works

1. Before the first fetch, `apply_layout` sets the filters, sort and collapse. A restored filter therefore also scopes that fetch.
2. The selection, list position and detail pane need data, so they wait in `App.resume`. The first load applies them from inside `set_data` / `replace_repo` (a clone-scoped start lands in the latter), so the detail pane follows through `nav()` and loads the comments.
3. The runs tab needs the clock, so the event loop switches to it after the load.
4. While the resume is pending, a snapshot reports the pending values. Without that, an early debounce or a quick quit would overwrite the saved selection with "nothing".
5. A selection that no longer exists (closed, moved) leaves the list at the top, with no message, as a refresh does.

## Notes

- Restoring `state: all` fetches every closed issue too, so an org-wide view saved that way is slow to load (about 30 seconds or more for pgmac-net). That is the view you left, resumed faithfully. Started in a clone, the same fetch is one repo and quick.
- The saver takes its directory as a parameter, so no test touches the real state directory.
