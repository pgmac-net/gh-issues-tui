# Tree view updates in place after a mutation (#196)

## Problem

After an issue was changed from the TUI, the list (tree view) on the left kept
showing the old data until the next refresh:

- Editing labels left the old label set on the row.
- Closing an issue while filtering for all issues left the green "open" marker.

## Cause

Every mutation finished with a full org refetch, and that refetch could read
data from before the write. Either a GitHub read replica had not yet applied
the write, or an auto-refresh started before the mutation landed after it. The
render path was not at fault: rows read `app.repos` directly, there is no HTTP
cache, and the tree caught up on the next refresh.

## What changed

| Mutation | Before | After |
|---|---|---|
| close / reopen, title, description, assignees, labels, priority | full org refetch | provider returns the updated issue; that one row is patched in place |
| add comment | full org refetch + thread refetch | thread refetch; listed comment count bumped locally |
| edit comment | full org refetch + thread refetch | thread refetch |
| create issue, move to another repo | full org refetch | unchanged (they change which issues exist or where) |

How each provider reports the updated issue:

- **GitHub:** `closeIssue` / `reopenIssue` / `updateIssue` select `issue { … }`
  (scalars plus `labels` and `assignees`). All `updateIssue` writes now share one
  mutation document taking an `UpdateIssueInput`.
- **Linear:** `issueUpdate` selects `issue { ...IssueFields }`, the same fragment
  the bulk fetch uses.
- **Jira:** writes answer 204, so each ends with `GET /issue/{key}` using the
  bulk fetch's field list and `map_issue`.

In the TUI:

- `with_issue_patch` spawns the mutation and sends `AppEvent::IssuePatched`.
- `App::patch_issue` (`src/tui/app/patch.rs`) replaces the entry, re-stamps
  label ranks, drops the issue's readiness judgement, rebuilds rows and
  reselects. The handler runs it inside `nav()` so the detail pane follows.
- If the patched issue no longer matches the filters, it leaves the list at
  once and the selection moves to its neighbour.
- If the issue left the list while the write was in flight, the status reads
  `… (no longer listed)`.
- `set_data` now merges via `keep_newer_loaded`. A loaded issue with a strictly
  newer `updated_at` beats the incoming copy, so a late or lagging refetch
  cannot roll a patch back.
- `CommentRefresh { Refetch, Skip }` became
  `AfterMutation { RefetchOrg, CommentAdded, CommentEdited }` for the mutations
  that report nothing back.

## Decisions

Recorded in `docs/adr/0005-mutations-patch-the-list-in-place.md`:

- all three providers, with the trait returning `Issue` (rather than an
  optimistic patch built from the request parameters, which has no server
  `updated_at` and guesses at server-side effects)
- no full refetch after issue-level mutations
- filters apply immediately
- the `updated_at` guard covers every issue, not only patched ones

## Deviations from the plan

- `CommentRefresh` was replaced by `AfterMutation` instead of being kept. With
  issue-level writes gone, the remaining callers differ by what follows them,
  and a comment add also needs the count bump.
- `MutationDone` gained `issue_id: Option<String>`. It is `None` for create,
  which reports a URL, not an id. The trait doc for `create_issue` wrongly said
  "node id" and now says "web URL".

## Tests

- `tui::app::tests`: patch touches one issue; unlisted patch is a no-op; close
  under `open` drops the row and clamps; close under `all` shows the new state;
  removing a filtered label drops the row; readiness is invalidated; comment
  count bump; `set_data` keeps the newer loaded copy, takes a newer refetch,
  and drops issues the refetch no longer returns.
- `tui::event::tests`: `IssuePatched` patches without an org refetch, and an
  unlisted patch says so. `MutationDone` refetches the org only for
  `RefetchOrg`; comment edit invalidates the thread cache; comment add bumps
  the count.
- `github::client::tests`: mutations select the full issue; payload parsing;
  missing issue is a shape error.
- `linear::client::tests`: `issueUpdate` payload parsing via the shared
  fragment; missing issue is a shape error.
