# Filter pushdown and loaded scope (#230)

Part 2 of #227. GitHub fetches now carry the filters they can, so a fetch brings back only issues that could be shown. Decision record: [ADR 0008](adr/0008-refreshes-are-scoped-and-pushdown-never-decides-what-is-shown.md). Terms: [CONTEXT.md](../CONTEXT.md#fetching-and-refreshing).

## What goes to the API

`App::wanted_scope()` builds a `FetchScope` from the filters, and GitHub sends it as `issues(filterBy: …)`:

| Filter | `IssueFilters` field | Notes |
|---|---|---|
| state (`f`), `--all` | `states` | always sent |
| assignee | `assignee` | a login that does not exist fails the whole query (`UNPROCESSABLE`), so the client retries once without it |
| author | `createdBy` | an unknown login just matches nothing |
| updated after | `since` | local midnight of the day, matching the client's local-day comparison |
| priority **or** status | `labels` | any-of; each value sent as `v` and `priority:v` (or `status:v`), since the client accepts both. Both groups set → neither is sent |

Checked live against pgmac-net:
- all of these match case-insensitively, as the client filter does
- `filterBy.labels` is any-of (43 + 17 = 60)
- `filterBy` inside the bulk org query still costs 1 point a page

Not sent: text search, created/closed dates, updated-before, and a substring repo filter. The client-side `Filters::matches` still runs on every loaded issue, so the shown list is identical with or without pushdown.

Linear and Jira accept `FetchScope` and use only `include_closed`.

## Loaded scope and auto-widen

- `App.loaded: Option<Loaded>` records the scope and the repo (`None` = org) the data was fetched for. A fetch records its target with `begin_fetch`, and `finish_fetch` promotes it when the fetch lands successfully. Partial `r` refreshes never begin one: they reuse `loaded_scope()`.
- `Loaded::covers(wanted)` decides whether the loaded data can serve the current filters.
- `refetch_if_widened` runs after every key and every event. It refetches the filtered scope once when the filters reach past `loaded`, and only when nothing is in flight, nothing is rate-limited, and the mode is Normal or the filter menu. Status: `filters widened, refreshing…`.
- Narrowing (e.g. adding an assignee) is instant. The next `Ctrl+R` or auto-refresh fetches at the narrower scope, which is when the payload shrinks.
- Showing closed issues is just a widen. The old one-off `include_closed` upgrade in `f` is gone.

## Startup inside a clone

Before anything has loaded, `filtered_target()` trusts the detected repo filter as exact, so startup fetches one repo (`loaded repo gh-issues-tui (10)` live, against 209 issues across 62 repos for the org). If that repo turns out not to exist, its group comes back gone, nothing matches exactly any more, and the auto-widen fetches the org. Clearing the filter later widens to the org the same way.

## Picker vocabulary

The filter pickers (repo, assignee, author, priority, status) used to list values from the loaded issues. After pushdown that would offer only what is already selected. `App.vocab` collects every repo, assignee, author and label from every fetch, patch and repo replace this session, and `switch_org` clears it. The inferred-priority options rank vocabulary labels with `label_rank.rank_of`, the same source the per-issue stamps use.

## Move targets

`m` used to offer the loaded repos. With one repo loaded that would be nothing, so the first `m` of a session fetches the org's repo names (`IssueProvider::repo_names`: GitHub repos-only query, Linear team keys, Jira project keys), shows `loading repos…`, and opens the picker when the list lands, if the same issue is still selected. The list is cached per org.

## Live verification (pgmac-net, tmux)

1. Start in the clone → `loaded repo gh-issues-tui (10)`, group expanded.
2. `F` → `c` → `refreshed org: 209 issues across 62 repos` (auto-widen).
3. Assignee `pgmac` via the picker → no fetch.
4. `Ctrl+R` → `refreshed org: 47 issues across 62 repos`; last-fetch cost 6 → 2 points.
5. `F` → `c` → back to 209 (auto-widen).
6. From a scoped start, `m` → `loading repos…` → picker of the org's other repos.

Found live and fixed: a scoped first load arrived through `replace_repo`, which folded the only group under `default_collapsed`. It now applies `set_data`'s single-visible-group rule.
