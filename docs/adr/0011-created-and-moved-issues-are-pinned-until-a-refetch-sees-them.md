# 11. Created and moved issues are pinned until a refetch sees them

## Status

Accepted (#249). Amends rule 4 and supersedes rule 5 of ADR 0005.

## Context

Creating an issue was the last mutation that still finished with a full org
refetch (ADR 0005 rule 5, `AfterMutation::RefetchOrg`). That caused both
symptoms in #249:

- The whole org was refetched even when one repo was filtered, and `r` was
  refused ("already refreshing") until it landed.
- The refetch is a read sent straight after the write. It could come back
  from a replica that did not have the new issue yet, so the issue only
  showed on the next auto-refresh, up to 5 minutes later. That is the same
  stale read that made ADR 0005 stop refetching after issue-level writes.

A move (#226) already placed its issue without a refetch, but it had the other
half of the problem: ADR 0005 rule 4 lets a refetch decide which issues exist,
so an auto-refresh or `Ctrl+R` landing from a replica that had not seen the move
dropped the moved issue until a later refresh.

## Decision

1. **Create returns the created issue.** `Provider::create_issue` returns
   `Result<Issue>`. GitHub's `createIssue` and Linear's `issueCreate` select the
   same fields as the other mutation payloads; Jira, whose create answers with a
   key alone, reads the issue back with `GET /issue/{key}`.
2. **It is inserted, not refetched.** `AppEvent::IssueCreated` →
   `App::insert_created` puts it at the top of its repo group (the group is
   created when it is not loaded), expands that group and selects the issue.
   When the filters hide it, the status says so and the selection stays.
   `AfterMutation::RefetchOrg` is gone.
3. **Created and moved issues are pinned.** `App.pins` records them by id.
   Where a refetch (`set_data` via `keep_newer_loaded`, or `replace_repo`) lacks
   a pinned issue, the loaded copy is put back in its group. This is the one
   exception to "membership is the refetch's to decide".
4. **A pin is released** when a refetch includes the issue, after
   `PIN_TTL` (10 minutes, pruned once per event-loop pass), when a refresh finds
   the issue gone (`remove_issue`) or its repo gone (`replace_repo` with `None`),
   and on `switch_org`.
5. **A create that cannot be read back is not a failure.** If Jira's follow-up
   read fails, `ProviderError::CreatedUnread { number }` says so; the status
   reads `created #N in repo — not shown yet, r to refresh` and the org is
   refetched, the one case that still does.

## Consequences

- A new issue shows the moment the API answers, `r` is never blocked by a
  create, and the create costs one call instead of an org fetch.
- An issue deleted or transferred elsewhere within 10 minutes of being created
  or moved here keeps showing until its pin expires or `r` on it finds it gone.
  Accepted: the window is short and the alternative is the bug.
- A pinned issue a pushed-down filter excludes from the fetch (an assignee
  filter, say) is kept by the pin but still hidden by `Filters::matches`, so
  nothing wrong is shown.
- Pins are keyed by id only. They hold no copy of the issue: the loaded copy
  is the one put back, and `keep_newer_loaded`'s newer-wins rule still applies
  to issues the refetch does include.
- Do not widen pins to patched issues: a patched issue is already in the list,
  and rule 4's `updated_at` comparison covers it.
