# 5. Mutations patch the list in place; refetches never regress an issue

## Status

Accepted (#196).

## Context

Every mutation used to finish with a full org refetch. `docs/architecture.md`
called that "correct by construction", but it isn't: the refetch is a separate
read, sent straight after the write, and it can come back with the old data.
Two ways this happens:

- GitHub's GraphQL reads can be served by a replica that has not yet applied
  the write.
- An auto-refresh started before the mutation can land after the
  mutation's own refetch, carrying data from before the write.

The symptom in #196 was a list row showing the old labels after an edit, and
the green "open" marker on an issue just closed under the `all` filter, until
the next refresh. The render path was fine: rows read `app.repos` directly, and
there is no HTTP cache.

The ticket also asked that the update preferably touch just the one issue
rather than the whole tree. The full refetch is the expensive org query plus
hydration batches, spent to learn about one issue.

## Decision

1. **Issue-level mutations return the post-write issue.** `set_state`,
   `update_title`, `update_body`, `set_assignees` and `set_labels` return
   `Result<Issue>` on every provider:
   - **GitHub:** the mutation payload selects `issue { … }` with the same
     scalars as the bulk query plus `labels`/`assignees`. A mutation response is
     consistent with its own write.
   - **Linear:** `issueUpdate` selects `issue { ...IssueFields }`, the bulk
     fetch's own fragment.
   - **Jira:** its write endpoints answer 204 with no body, so each one ends
     with `GET /issue/{key}`, mapped by the bulk fetch's `map_issue`.
2. **That one entry is patched in place, and the org is not refetched.**
   `AppEvent::IssuePatched` → `App::patch_issue` replaces the issue by id, then
   re-stamps label ranks, drops that issue's readiness judgement (derived from
   the old text), rebuilds rows and reselects, all inside `nav()`.
3. **Filters apply at once.** A patched issue that no longer matches (closed
   under `open`, a filtered label removed) leaves the list immediately, and the
   selection clamps to its neighbour, as on any data change. There is no
   "sticky row" exception.
4. **A refetch never regresses an issue.** `set_data` keeps the loaded copy of
   any issue whose `updated_at` is strictly newer than the incoming copy's.
   This applies to every issue, with no record of which were patched and no
   expiry window. Equal timestamps take the incoming copy. Issues absent from
   the refetch stay absent, because membership is always the refetch's call.
5. **Only mutations that change membership still refetch the org.** Create
   and move-to-another-repo use `AfterMutation::RefetchOrg`. Comment add/edit
   refetch only the thread; an add bumps the listed `comment_count` locally.

## Consequences

- No more stale rows after a write, and one fewer org fetch per edit.
- Changes other people make are still picked up by auto-refresh and `r`, as
  before. A mutation no longer doubles as a refresh of everything else.
- Rule 4 trusts `updated_at` to be monotonic per issue. GitHub, Linear and Jira
  all set it server-side on every write. A backend that does not would have a
  real change hidden until a later refresh brings an even newer timestamp.
- Jira pays one extra `GET` per issue-level write. It is still far cheaper than
  the org refetch it replaces.
- GitHub mutation payloads carry `labels`/`assignees` connections. This does
  not contradict the bulk query's no-nesting rule (`docs/graphql-api-cost.md`):
  that cost is per issue slot across a paginated connection, and a mutation
  returns one node.
- Do not add a "just to be safe" org refetch after a patch. That refetch is
  the read that went stale.
