# Context

Terms used across this repo's code and docs.

## Fetching and refreshing

**Refresh scope**: how much of the list a refresh refetches. Three sizes: one **issue** (`r` on an issue row), one **repo** (`r` on a repo header), and the **filtered scope** (`Ctrl+R` and auto-refresh). *Avoid*: "reload", which used to mean the whole org every time.

**Filtered scope**: the one repo an exact repo filter names, otherwise the whole org. A substring repo filter does not narrow it.

**Fetch scope** (`FetchScope`): the parameters a fetch sends to the backend: `include_closed`, assignee, author, updated-since and one label group (#230). *Avoid*: "filter" alone, which means the client-side `Filters` that decide what is shown.

**Loaded scope** (`App.loaded`): the fetch scope the loaded data was fetched with, plus the one repo it was narrowed to, if any. Partial refreshes reuse it, so one group is never fresher in kind than the others.

**Vocabulary** (`App.vocab`): every repo, assignee, author and label seen in any fetch this session, which the filter pickers offer. *Avoid*: "loaded labels", which since #230 is only what matches.

**Widen** and **narrow**: a filter change widens when it could show issues outside the loaded scope (clearing an assignee, showing closed issues), and narrows when it can only hide loaded ones. Narrowing never fetches; widening refetches the filtered scope once.

## Workflow runs

**Linked PR**: a pull request connected to an issue, by the issue's timeline (a cross-reference or a connected PR) or by a link in the issue's body or comments. Mentioning is enough; closing is not required. *Avoid*: "the issue's PR", since an issue can have several.

**Run**: one GitHub Actions workflow run. The tab shows a PR's runs for its current head SHA, newest per workflow and trigger event.

**Attempt**: one execution of a run. Re-running a run makes a new attempt of the same run, and the API returns the latest attempt. Not a new run.

**ETA history**: the last 10 successful runs of the same workflow and trigger event, which a forecast is learned from. *Avoid*: "cache", which is how it is held, not what it is.

**Forecast**: the figures that look ahead: time remaining (`~2m54s`, `~2m54s+`) and the ETA clock (`ETA 13:01`). Drawn in `runs_eta`, bold by default. *Avoid*: "ETA" for the whole group, since the clock time is only one part. `blocked` and `overdue` are problems, not forecasts, and use `warning`.

**Load** and **poll**: a load finds the PRs from scratch; a poll re-reads the PRs already known. *Avoid*: "refresh" for either. `r` is a refresh, and it does a load.

## Detail pane

**Fold** is comments and tree nodes in the detail pane. **Collapse** is repo groups and sub-issue parents in the list.

## Sub-issues (#234)

**Sub-issue**: an issue with a parent issue, which nests under it in the list. It is the only link that nests; dependencies (blocked by / blocking) and related links are counted in the detail pane. *Avoid*: "linked issue" for a sub-issue, since that covers all three.

**Ancestry**: an issue's parent, grandparent, and so on, nearest first (`Issue.ancestors`), fetched with the issue.

**Context row**: an ancestor drawn dim above its sub-issues because it is not itself a visible issue of the group: filtered out, in another repo, or not loaded. Never counted. *Avoid*: "ghost row".

**Stub**: a context row whose issue is not loaded, known only from a descendant's ancestry. Read-only; `o` opens it.
