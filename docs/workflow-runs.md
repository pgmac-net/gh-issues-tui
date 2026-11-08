# Workflow runs tab

Tracks [gh-issues-tui#222](https://github.com/pgmac-net/gh-issues-tui/issues/222).
Decision record: [ADR 0007](adr/0007-the-runs-tab-is-read-only-and-polls-only-while-visible.md).
Terms: [CONTEXT.md](../CONTEXT.md). Every run of a repo, from its header: [repo-workflows.md](repo-workflows.md) (#266).

## What it does

The lower detail region has two tabs, `comments` and `runs`. `R` flips between them, from the list or from the pane; from the list with the pane closed it opens the pane on the runs tab. The tab is GitHub only: on Linear and Jira the region keeps its plain `comments` title and `R` reports "not supported".

```text
┌ comments │ runs (R) ──────────────────────────────────────────────┐
│▾ ● #223 feat(runs): workflow runs tab                      ~2m54s │
│  ▾ ● CI · pull_request            2/4 · 1m52s · ~2m54s · ETA 13:01 │
│    ▸ ✓ fmt                                                     5s │
│    ▾ ● CI (windows-latest)                 4/8 · 1m50s · ~2m54s │
│        ✓ Set up job                                            1s │
│        ● clippy                                             1m33s │
│        · test                                                     │
└───────────────────────────────────────────────────────────────────┘
```

Each row is one screen row: the name truncates, the figures on the right stay whole. `✓` success, `✗` failed, `●` running, `!` waiting, `·` queued, `-` skipped or neutral. `2/4` is jobs done on a run and steps done on a job. Active and failed items start unfolded; finished successes collapse to one line.

## Keys

The tab follows the pane-key rules from ADR 0006 and acts only when the detail pane has focus.

| Key | Action |
|---|---|
| `R` | flip between comments and runs |
| `↑`/`↓`, `PgUp`/`PgDn`, `g`/`G` | move the row cursor |
| `→` | unfold, then step in to the first child |
| `←` | fold, then step out to the parent, then out to the list |
| `Space`/`Enter` | toggle the fold |
| `[` / `]` | fold or unfold everything |
| `Tab`/`Shift+Tab` | next or previous PR or run header |
| `o` | open the selected PR, run or job on GitHub (a step opens its job) |
| `r` | refresh: find the PRs again, retry workflows with no history |
| `c` | back to comments and start a comment |

`e` does nothing on the tab, because no card is highlighted.

## Which runs

**Linked PRs** are the union of the issue's timeline (cross-references and connected PRs) and links in its body and comments, parsed the same way `P` does. The timeline matters because a PR that references an issue from its own body leaves no link in the issue. `Closes #N` shows up as a cross-reference, so it needs no separate query. References that turn out to name an issue, or nothing, are dropped; so are links to repos the token cannot see, since GitHub answers both the same way. At most 20 unknown text references are resolved per issue.

**Open and merged PRs only**, open first, then merged newest first, capped at 5 with a `+N more` line. Closed-unmerged PRs are dropped and not counted.

**Runs** are those of the PR's current head SHA, newest per workflow and trigger event, so an older push or an earlier re-run does not appear. Push and pull-request runs of one workflow are different runs and both show.

Checks from outside Actions do not appear.

## ETA

`src/ci/estimate.rs` is a port of gh-eta's estimator, with its tests; [gh-eta's how-it-works](https://github.com/pgmac/gh-eta/blob/main/docs/how-it-works.md) has the model and measured accuracy. History is the last 10 successful runs of the same workflow and event.

| Shown | Meaning |
|---|---|
| `~2m54s · ETA 13:01` | time left and clock time, for the run |
| `~2m54s+` | as above, but a job with no history is left out |
| `blocked +19s` | waiting for an environment approval; needs that long once approved |
| `overdue` | past its P90, with nothing in its history saying how much longer |
| `?` | a job that has never succeeded before |
| `no ETA` | no successful past runs to compare with |

## Colours (#240)

The figures on the right of each row are coloured by what they are, so the forecast stands out from the time taken. All of it is set in a `[color_profiles.<name>]` table (see the README), and each default follows an existing colour as your profile sets it.

| Figure | Key | Default |
|---|---|---|
| Forecast: `~2m54s`, `~2m54s+`, `ETA 13:01`, on run, job and PR rows | `runs_eta`, plus `runs_eta_bold` | `warning` (yellow), bold |
| Time taken, live or finished: `1m58s`, `12s` | `runs_elapsed` | `dim`, as before |
| Done/total counts: `1/2` jobs, `3/5` steps | `runs_progress` | `open` (green) |
| `blocked +19s`, `overdue` | `warning` | yellow |
| `?`, `no ETA`, separators, conclusion words (`failure`, `cancelled`), PR summaries (`2 runs`, `1/3 running`, `2/3 failed`, `merged`) | `dim` | dark grey |

Blocked and overdue are problems rather than forecasts, so they use `warning` itself and don't change with `runs_eta`. A PR row's counts stay dim: its glyph already shows its state, and a green `2/3 failed` would read wrong. The selected-row highlight is on the name only, so the figures keep their colours.

Against `gh eta` on a live run of this repo's CI: `~3m45s` at 1m01s in the tab, `~3m41s` at 1m05s from `gh eta`, both ETA 13:01.

## Refresh and cost

| Call | Cost | When |
|---|---|---|
| Timeline | 1 GraphQL, plus 1 per unknown text link | on load and `r` |
| Runs of a head SHA | 1 REST per PR | on load; open PRs each poll |
| Jobs of a run | 1 REST per run | active runs each poll; finished runs once |
| ETA history | about 11 REST per workflow+event | once per session |

A poll fires every 15 seconds only while the tab is showing an active run on an open PR. It stops when they finish, when the tab is flipped away, and behind a full-frame harness session. A poll re-reads the PRs it already knows, so a PR opened or pushed while polling appears on `r`. Merged PRs are never polled.

Moving the list selection loads after 250 ms of quiet, so walking the list costs nothing until it stops. Times on screen move with each poll, not each second, because rendering is a pure function of `App`.

Polling holds below 100 remaining REST requests until the budget resets, and says so in the region's bottom border. REST has its own rate-limit store: the info bar stays on the GraphQL budget.

A fine-grained token needs Actions read on the repo. Without it the PR row shows `unreadable` and the reason under it, and the other PRs still load.

## Code

| Where | What |
|---|---|
| `src/ci/mod.rs` | backend-neutral run, job and step types, `RunStatus`, `CiProvider` (#266, ADR 0014) |
| `src/ci/estimate.rs` | estimator (pure) |
| `src/runs/mod.rs` | linked PRs, PR filter, newest-run rule |
| `src/runs/fetch.rs` | `load` and `poll`: provider calls, per-PR failure |
| `src/github/actions.rs` | timeline query and REST reads |
| `src/tui/app/runs.rs` | `RunsState`, scheduling |
| `src/tui/app/run_tree.rs` | tree rows and cursor |
| `src/tui/ui/runs.rs` | rendering |

`App` holds `due`; the event loop sleeps until it and calls `App::runs_tick`, which decides whether to fetch. A landed response is the only thing that re-arms a poll, so two fetches for one issue cannot overlap, and every response carries a generation so one overtaken by a selection change or `r` is dropped.
