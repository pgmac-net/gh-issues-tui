# Granular refresh (#229)

Part 1 of #227. Before this, every refresh refetched the whole org. Now a refresh is the size of what you are looking at. Decision record: [ADR 0008](adr/0008-refreshes-are-scoped-and-pushdown-never-decides-what-is-shown.md). Terms: [CONTEXT.md](../CONTEXT.md#fetching-and-refreshing).

## Keys

| Key | Context | Refetches | Status line |
|---|---|---|---|
| `r` | issue row | that issue; its thread too if the detail pane shows it | `refreshed #42` / `#42 no longer exists, removed` |
| `r` | repo header | that repo's issues | `refreshed repo foo (12)` / `repo foo is gone, archived or has issues off, removed` |
| `r` | runs tab focused | the runs (unchanged, #222) | as before |
| `Ctrl+R` | anywhere in the list | the **filtered scope**: the exact-filtered repo, else the org | `refreshed repo foo (12)` / `refreshed org: 87 issues across 9 repos` |
| (timer) | auto-refresh | the org (since #226, ADR 0010) | `auto-refreshed org: …` |

The first load of an org still says `loaded N issues across M repos`. A refresh key pressed while a refresh is running says `a refresh is already running` and starts nothing.

## Provider surface

Two required `IssueProvider` methods:

- `issue(id) -> Result<Option<Issue>>`: the issue with labels and assignees.
  - GitHub: `node(id:)` with the mutation payload's field set (`full_issue_fields!`).
  - Linear: `issue(id:)` with `IssueFields`.
  - Jira: `GET /issue/{key}` (the read mutations already used).
- `repo_issues(org, repo, scope) -> Result<Option<RepoIssues>>`: one group, shaped like its `org_issues` entry. (Since #230 it takes a `FetchScope`; it took `include_closed` here.)
  - GitHub: `REPO_QUERY` (first page, plus `isArchived`), then the same page loop and hydration as the bulk fetch (`finish_repo`).
  - Linear: team by key, then the shared `team_issues` loop.
  - Jira: `GET /project/{key}` to tell "gone" from "empty", then the shared `project_issues` JQL loop.

`Ok(None)` means gone. Each backend reports it its own way:
- GitHub: an all-`NOT_FOUND` error list (checked live)
- Linear: `Entity not found: …` (from Linear's docs; not yet seen live)
- Jira: HTTP 404

Each is classified as `ProviderError::NotFound` and turned into `None` by `provider::error::found`. `NotFound` displays exactly like the `Api` error it used to be.

## Folding answers back

- **Issue:** `AppEvent::IssueRefreshed` → `App::patch_issue` inside `nav()`, the same path as a mutation (#196), so a lagging read cannot regress a newer copy. `None` → `App::remove_issue`.
- **Repo:** `AppEvent::RepoRefreshed` → `App::replace_repo`. Only that repo changes; within it the refetch decides membership and `keep_newer_loaded` keeps newer patched copies. The repo's cached threads and readiness judgements are dropped. A selected header stays selected; a selected issue stays selected by id. `None` drops the group.
- Partial refreshes use the loaded scope (`include_closed` here; the full `FetchScope` since #230), so the list stays one dataset.

## Not changed

- Create and move still fetch the org. Since #230, startup and showing closed issues go through the filtered scope and the auto-widen instead: see [filter-pushdown.md](filter-pushdown.md).
- The info bar's "last fetch" cost is the last **org** fetch's; partial refreshes do not update it.
