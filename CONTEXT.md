# Context

Terms used across this repo's code and docs.

## Fetching and refreshing

**Refresh scope**: how much of the list a refresh refetches. Three sizes: one **issue** (`r` on an issue row), one **repo** (`r` on a repo header), and the **filtered scope** (`Ctrl+R` and auto-refresh). *Avoid*: "reload", which used to mean the whole org every time.

**Filtered scope**: the one repo an exact repo filter names, otherwise the whole org. A substring repo filter does not narrow it.

**Fetch scope** (`FetchScope`): the parameters a fetch sends to the backend: `include_closed`, assignee, author, updated-since and one label group (#230). *Avoid*: "filter" alone, which means the client-side `Filters` that decide what is shown.

**Loaded scope** (`App.loaded`): the fetch scope the loaded data was fetched with, plus the one repo it was narrowed to, if any. Partial refreshes reuse it, so one group is never fresher in kind than the others.

**Vocabulary** (`App.vocab`): every repo, assignee, author and label seen in any fetch this session, which the filter pickers offer. *Avoid*: "loaded labels", which since #230 is only what matches.

**Widen** and **narrow**: a filter change widens when it could show issues outside the loaded scope (clearing an assignee, showing closed issues), and narrows when it can only hide loaded ones. Narrowing never fetches; widening refetches the filtered scope once.

## Caching (#226)

**Snapshot**: the last loaded issues of one org on one backend, with the loaded scope they were fetched with, kept on disk and drawn at startup before the first fetch lands. Read-only: writes go to the backend and the snapshot follows. *Avoid*: "cache" alone, which also covers the Jev answer caches; "offline copy", since nothing can be edited from it.

**Usable snapshot**: one whose scope covers the wanted scope. Any other snapshot is treated as missing.

**Settle**: fold an answer from the backend (a load, refresh, patch or move) into the list. Each settle is what the snapshot saves.

**Jev answer cache**: a readiness or search answer kept on disk under a hash of the exact state sent, plus model and prompt version. Stale exactly when that input changes. *Avoid*: "Jev cache", which would include the label-rank cache, a separate thing with its own lifecycle.

**Load** (of the list): fetch data the list does not hold yet: the first load, or a widen past the loaded scope. A **refresh** refetches data it already holds. *Avoid*: calling a widen a refresh.

## Workflow runs

**Linked PR**: a pull request connected to an issue, by the issue's timeline (a cross-reference or a connected PR) or by a link in the issue's body or comments. Mentioning is enough; closing is not required. *Avoid*: "the issue's PR", since an issue can have several.

**Run**: one GitHub Actions workflow run. The tab shows a PR's runs for its current head SHA, newest per workflow and trigger event.

**Attempt**: one execution of a run. Re-running a run makes a new attempt of the same run, and the API returns the latest attempt. Not a new run.

**ETA history**: the last 10 successful runs of the same workflow and trigger event, which a forecast is learned from. *Avoid*: "cache", which is how it is held, not what it is.

**Forecast**: the figures that look ahead: time remaining (`~2m54s`, `~2m54s+`) and the ETA clock (`ETA 13:01`). Drawn in `runs_eta`, bold by default. *Avoid*: "ETA" for the whole group, since the clock time is only one part. `blocked` and `overdue` are problems, not forecasts, and use `warning`.

**Load** and **poll**: a load finds the PRs from scratch; a poll re-reads the PRs already known. *Avoid*: "refresh" for either. `r` is a refresh, and it does a load.

**Repo workflows view** (#266): what the detail pane shows while a repo header is selected: every recent run of that repo, grouped and filtered. *Avoid*: "runs tab", which is the issue-scoped region of #222.

**Page** (of runs): one list call's worth, 100 runs, newest first. The view starts with one; `L` adds the next. A poll re-reads every loaded page.

**Run filter**: status, workflow, event, branch and actor, one value each, sent to the CI backend with the list call; plus a text search that never leaves the client. Per repo, session-only. *Avoid*: "filter" alone, which means the issue list's `Filters`.

**Grouping** (of runs): one level, `flat`, `status`, `workflow` or `branch`, cycled with `v`. Saved in the layout.

**CI provider** (`ci::CiProvider`): the backend that runs workflows. Separate from the issue provider, though GitHub is both. *Avoid*: "Actions" for the abstraction; it is one backend.

**Run status** (`RunStatus`): a run's, job's or step's state in backend-neutral words: queued, waiting, running, success, failure, cancelled, skipped, neutral. *Avoid*: reading GitHub's `status`/`conclusion` strings outside the GitHub mapping.

**Completed-run cache**: jobs of finished runs on disk, keyed by run and attempt (ADR 0014). The run list itself is never cached.

## Detail pane

**Fold** is comments and tree nodes in the detail pane. **Collapse** is repo groups and sub-issue parents in the list.

## Layout (#236)

**Layout**: where you were in a view: filters, sort, collapse, the selected issue and the detail pane. Saved per view and resumed on the next start. *Avoid*: "state" (too broad: fetched data, popups and sessions are also state, and none of them are saved) and "session" (that is a harness session).

**View**: the org browsed plus the clone the app was started in, if any. One layout file per view.

**Resume**: apply a saved layout at startup. The parts that don't need data (filters, sort, collapse) apply at once, before the first fetch; the selection and the detail pane wait for the first load.

## Sub-issues (#234)

**Sub-issue**: an issue with a parent issue, which nests under it in the list. It is the only link that nests; dependencies (blocked by / blocking) and related links are counted in the detail pane. *Avoid*: "linked issue" for a sub-issue, since that covers all three.

**Ancestry**: an issue's parent, grandparent, and so on, nearest first (`Issue.ancestors`), fetched with the issue.

**Context row**: an issue drawn dim in a group where it is not a visible issue, to keep the tree whole. Two kinds: an ancestor above its sub-issues (filtered out, in another repo, or not loaded), and a **cross-repo descendant** below a visible parent (#260): a loaded sub-issue from another repo that passes every filter but the repo filter. Never counted. *Avoid*: "ghost row".

**Home row**: an issue's own row, in its own repo's group. A context row from another repo is a link to it: `Enter` jumps there (#260); `←`/`→`/`Space` stay on the dim row.

**Outside sub-issue**: a sub-issue fetched for its parent because it is not in the loaded scope (another owner or repo not fetched, closed while showing open, outside a pushed-down filter) (#261). A full issue, filtered like any other, held in `App.outside` apart from `App.repos`: drawn as a context row, never counted, read-only (`o` and `Enter` open it). *Avoid*: "stub", which has only what a descendant's ancestry carries; "loaded", which still means inside the loaded scope.

**Anchor**: the visible issue of a group that a cross-repo descendant hangs under. Only visible issues anchor; a context row never does, so an epic drawn as context does not pull its whole tree into the group.

**Stub**: a context row whose issue is not loaded, known only from a descendant's ancestry. Read-only; `o` opens it.

## Refresh indicator (#250)

**Refresh indicator**: the one-column spinner at the left of the info bar while `App.loading` is true. Glyph steps every 100 ms; colour is a rainbow hue of the time since loading began. *Avoid*: "loading spinner" or "progress bar" (it shows that data is loading, not how far along it is), and "refresh" for the runs-tab poll, which does not light it.

**Revolution**: one pass through the indicator's whole glyph set. `spinner_hue_revolutions` revolutions make one rainbow cycle.

**Config warning**: a startup problem with a config value that falls back instead of failing (so far the `spinner_*` keys). Printed to stderr and held on the status line until the first keypress.

## Pins (#249)

**Pin**: a mark on an issue this session created or moved, held until a refetch has included it (or 10 minutes pass, a refresh finds it gone, or the org changes). A refetch that lacks a pinned issue keeps it. *Avoid*: "optimistic insert" — the pinned issue is the copy the backend reported, never a guess.
