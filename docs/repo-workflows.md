# Repo workflows view

Tracks [gh-issues-tui#266](https://github.com/pgmac-net/gh-issues-tui/issues/266).
Decision record: [ADR 0014](adr/0014-ci-is-its-own-provider-and-only-completed-jobs-are-cached.md), with the polling rules of [ADR 0007](adr/0007-the-runs-tab-is-read-only-and-polls-only-while-visible.md).
Terms: [CONTEXT.md](../CONTEXT.md). The issue-scoped runs tab it reuses: [workflow-runs.md](workflow-runs.md).

## What it does

With the detail pane open and a **repo header** selected, the whole pane shows that repo's recent workflow runs, grouped and filtered. Move to an issue row and the pane is the issue again, runs tab and all. `R` on a header with the pane closed opens straight into the view. It is GitHub only; on Linear and Jira a header still shows "no issue selected".

```text
┌ workflows · gh-issues-tui · group: status (v) ────────────────────────────┐
│▾ ● running                                                         1 run │
│  ▾ ● CI #684 · 266-repo-workflows-view · push  2/4 · 1m52s · ~2m54s · ETA│
│    ✓ fmt                                                              5s │
│    ● test (ubuntu-latest)                           4/8 · 1m50s · ~2m54s │
│▾ ✗ failed                                                         2 runs │
│  ▸ ✗ CI #575 · renovate/vt100-0.x · pull_request  failure · 2m32s · 20d …│
│▸ ✓ success                                                       97 runs │
│    +619 more (L)                                                         │
└ 100 of 719 ──────────────────────────────────────────────────────────────┘
 Update Rust crate unicode-width to v0.2.3 — renovate[bot]
```

Rows are the issue tab's: one screen row each, the name truncated, the figures whole. Jobs, steps, glyphs, the forecast (`~2m54s`, `ETA 13:01`, `blocked`, `overdue`, `?`, `no ETA`) and the `runs_*` colours are drawn by the same code. A run's name is `workflow #number · branch · event`, less whatever the view is grouped by. A finished run adds how long ago it was created. The selected run's title and actor go on the bottom line, ahead of any status message.

## Keys

The view follows the pane-key rules of ADR 0006 and acts only with the detail pane focused. With the list focused, `f`, `F` and `/` keep their list meanings.

| Key | Action |
|---|---|
| `↑`/`↓`, `PgUp`/`PgDn`, `g`/`G` | move the row cursor |
| `→` | unfold (a finished run reads its jobs), then step in |
| `←` | fold, step out to the parent, then out to the list |
| `Space`/`Enter` | toggle the fold |
| `[` / `]` | fold or unfold everything loaded (unfolding does not read every finished run's jobs) |
| `Tab`/`Shift+Tab` | next or previous group header (runs, when flat) |
| `v` | grouping: status → workflow → branch → flat |
| `f` or `F` | filter menu: status, workflow, event, branch, actor |
| `/` | text search over workflow, run title, branch, actor and `#number` |
| `L` | load the next 100 runs |
| `r` | refresh: every loaded page, the workflow list, and workflows with no history |
| `o` | open the selected run or job (a step opens its job; a group opens the repo's Actions tab narrowed to it) |
| `R` | says the view follows the header: there is no tab to flip |

## Which runs

The newest 100 runs of the repo, every run, not only the newest per workflow. `L` adds the next 100. The bottom border says how many are loaded of how many match (`100 of 719`).

**Grouping** is one level. `status` (the default) orders running, waiting, queued, failed, success, other. `workflow` and `branch` are ordered by their newest run. A group starts unfolded when it holds an active or failed run. The grouping is saved in the layout and comes back on the next start.

**Filters** take one value each, because that is what GitHub can filter on, and are sent with the list call, so a filtered view reaches 100 matching runs, not whatever matched in the newest 100. They are applied on the client too: GitHub has no `startup_failure` filter, so asking for it fetches every failure and the client narrows. Status values are fixed; workflows come from the repo's workflow list, read once a session; events, branches and actors are those seen in the repo's runs this session, and a branch, event or actor never seen can be typed into the picker. The text search never leaves the client. Filters are per repo and session-only; the config's `[workflows]` table supplies their [starting values](#starting-state).

## Starting state

Tracks [gh-issues-tui#271](https://github.com/pgmac-net/gh-issues-tui/issues/271). The optional `[workflows]` table in `config.toml` sets where the view starts:

```toml
[workflows]
group = "branch"     # flat | status | workflow | branch (default: status)
status = "failure"   # a run status word, as in the filter menu
event = "push"
branch = "main"
actor = "pgmac"
search = ""          # text search
```

- **`group`** beats the saved layout's grouping on every start, and again after `w` switches org. `v` still changes it for the session, but that choice is not carried to the next start while `group` is set. Unset, the saved grouping comes back as before.
- **Filters** are the default for every repo's view until you change that repo's filter in the session. Any edit, including clearing in the filter menu, wins from then on: a cleared filter stays clear rather than falling back to the config. They are sent with the first list call like any other filter.
- An unknown `group` or `status` word is ignored with a startup warning. There is no workflow-name filter in config (the workflow list is read lazily per repo), and no per-repo overrides.

## Refresh and cost

| Call | Cost | When |
|---|---|---|
| Run list | 1 REST per loaded page | on entering the view, on `r`, each poll |
| Workflow list | 1 REST per 100 workflows | once a session per repo, and on `r` |
| Jobs of an active run | 1 REST per run | every read while it is active, once more when it finishes |
| Jobs of a finished run | 1 REST | when it is first unfolded, then from disk |
| ETA history | about 11 REST per workflow+event | once a session, shared with the runs tab |

Entering a header reads it after a 250 ms debounce, so walking past headers costs nothing. Polling follows ADR 0007: every 15 seconds while the view is showing and a loaded run is active, re-reading every loaded page; paused behind a harness session and below 100 remaining REST requests (said in the bottom border); only a landed read arms the next. Leaving the header drops its runs and pages; coming back reads them again.

**Completed-run cache.** Jobs of finished runs are written to `run_jobs.json` in the org's cache directory, keyed by owner, repo, run id and attempt, at most 2000 runs, oldest written first out. The attempt is in the key, so a re-run never shows the old attempt's jobs. The run list itself is never cached. `cache = false` turns it off and `--no-cache` wipes it with the rest of the org's cache; it holds no issue text, so the Jev consent does not gate it.

## Plugging in another CI system

The CI half of a backend is `ci::CiProvider` (ADR 0014), separate from `IssueProvider`, which exposes it through `IssueProvider::ci()`. Everything past the edge reads backend-neutral types (`CiRun`, `CiJob`, `CiStep`) and a `RunStatus` enum; GitHub's `status`/`conclusion` words are mapped once, in `github/actions.rs`. A new backend implements the trait, maps its own words, and says in `capabilities()` which filters it can send; the menu offers only those. `runs_page_url` is optional.

## Code

| Where | What |
|---|---|
| `src/ci/mod.rs` | neutral types, `RunStatus`, `RunFilter`, `CiProvider` |
| `src/ci/estimate.rs` | estimator (pure), moved from `runs/` |
| `src/ci/repo.rs` | `read_pages`: list pages, jobs for active runs, history |
| `src/github/actions.rs` | GitHub's mapping, `list_runs`, `list_workflows`, the Actions tab URL |
| `src/cache/run_jobs.rs` | the completed-run cache |
| `src/tui/app/repo_runs.rs` | `RepoRunsState`, scheduling, landing |
| `src/tui/app/repo_run_tree.rs` | grouping, client-side filter, rows and cursor |
| `src/tui/app/run_filter.rs` | the filter menu and pickers |
| `src/tui/ui/repo_runs.rs` | rendering, reusing `ui/runs.rs`'s row helpers |
