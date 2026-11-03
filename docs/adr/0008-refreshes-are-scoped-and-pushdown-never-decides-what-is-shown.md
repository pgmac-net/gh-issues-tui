# 8. Refreshes are scoped, and pushdown never decides what is shown

## Status

Accepted (#227, built in #229 and #230). Rule 1 is partly superseded by ADR 0010 (#226): auto-refresh now covers the org.

## Context

Every refresh used to refetch the whole org: `r`, the auto-refresh ticker, showing closed issues, creating or moving an issue. #227 asks for refreshes the size of what you are looking at, and for filtering at the API instead of in the app. A full org fetch already costs only about 4 GraphQL points (#107), so the gains are payload size, latency and fewer hydrate batches more than budget. Also, once a filter changes what is fetched, a filter change can need a fetch where before it was instant.

## Decision

1. **Three refresh scopes.** `r` refreshes the selected issue or repo group; `Ctrl+R` and auto-refresh refresh the filtered scope: the one repo an exact repo filter names, otherwise the org. A substring repo filter does not narrow the fetch, since N single-repo queries cost more than one bulk query.
2. **A one-issue answer is a patch.** It goes through `patch_issue` like a mutation's result, so `keep_newer_loaded` still holds. A one-repo answer replaces only that repo, and within it the refetch decides membership as `set_data` does for the org.
3. **Gone is an answer, not an error.** An issue or repo the backend no longer resolves comes back as `Ok(None)` (`ProviderError::NotFound` mapped by `provider::error::found`) and is removed with a status line.
4. **Pushdown only shrinks the payload (#230).** The client-side `Filters::matches` still runs on every loaded issue. A pushed-down filter can leave out issues that would be hidden anyway, never decide on its own what is shown.
5. **Partial refreshes use the loaded scope**, not the current filters, so the list stays one consistent dataset. Widening past it is a full refetch of the filtered scope.
6. **One refresh at a time.** A refresh key pressed while one is out says so and starts nothing, so two answers cannot race to settle the list.

## Consequences

- A refresh of one repo leaves the other repos as old as they were. `Ctrl+R` with no repo filter is the way to bring everything up to date.
- The single-repo and single-issue reads are required trait methods: Linear (team by key, `issue(id:)`) and Jira (`/project/{key}`, `/issue/{key}`) implement them too, so `r` means the same on every backend.
- Create and move still refetch the org, because they change which issues exist or where.
