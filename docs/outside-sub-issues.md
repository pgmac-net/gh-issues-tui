# Outside sub-issues (#261)

A parent's sub-issues that the app hasn't loaded are fetched for it and drawn dim under it, like the loaded ones from #260. Decision record: [ADR 0013](adr/0013-outside-sub-issues-are-fetched-whole-per-refresh-and-kept-apart.md). Term: *outside sub-issue* in [CONTEXT.md](../CONTEXT.md). User-facing behaviour: [sub-issues.md](sub-issues.md).

## Problem

#260 showed only **loaded** sub-issues, because the tree is built from each issue's ancestry and the bulk query cannot carry a `subIssues` connection. Missing were sub-issues in another owner, in a repo not fetched (starting inside a clone fetches only that repo), closed ones while showing open, and ones outside a pushed-down filter.

## Decisions (from the ticket's grilling)

| Question | Decision |
|---|---|
| Stub or full issue | Full issue, labels and assignees hydrated, so every filter but the repo filter applies, as to a loaded child. |
| When | One eager batch after each load or refresh that covers the parent, for parents whose `sub_issues.total` is more than the sub-issues loaded for them. |
| How long trusted | Until the next covering refresh, whose answer replaces them; the old ones stay drawn until it lands. |
| Snapshot | Saved with it, drawn at startup, not part of `usable_for`. |
| Depth | Down the tree, level by level, to GitHub's 8-level limit. |
| Keys | `Enter`/`o` open in the browser; `→`/`Space` detail with comments; write keys read-only; `y` its own owner/repo; `r` refreshes the group. |
| Backends | GitHub only; Linear and Jira are #263. |

## How

- `Provider::sub_issues(parent_ids)` (default unsupported, probe `supports_sub_issues`). GitHub sends `SUB_ISSUES_QUERY`, 25 parents per request, each `subIssues(first: 50)` with the list's scalar fields plus `repository { name owner { login } }`, then hydrates labels and assignees through the bulk list's own `hydrate_issues`.
- `App.outside` (`app/outside.rs`) holds one entry per parent: every sub-issue the backend reported, loaded ones too, so a child that becomes loaded is drawn once, at home.
- `sub_issue_batch(scope)` runs after `Data` (all), `RepoRefreshed` (that repo) and `IssueRefreshed` (that issue). It also drops entries of parents in scope that are no longer short, and prunes entries no loaded issue leads to.
- `apply_sub_issues` drops an answer older than one already applied for the same parent (generations), replaces the rest, and returns the next level: fetched children that are themselves short.
- In `rebuild_rows`, outside sub-issues not also loaded are indexed by id, and the ones that pass the filters are anchored on a visible ancestor, like #260's cross-repo children (`Src::Outside`, `ContextRef::Outside`). One sort rank covers loaded and outside issues. An ancestor that is an outside sub-issue is now drawn from it rather than as a stub.
- Rows index into the store, so every change goes through `outside_changed`: rebuild, `RowKey` reselect, `data_gen` bump for the snapshot.

## Cost

Measured 1 point for one parent. A full batch of 25 parents is about 13, and a refresh with no short parent sends nothing. See [graphql-api-cost.md](graphql-api-cost.md#outside-sub-issues-261).

## Verification

- 20 new tests: batch selection and scopes, drawing and counts, filters with the repo filter ignored, loaded copy wins, next level and depth limit, late answers dropped, replace on refresh, pruning, read-only keys, own owner/repo for `y` and PR shorthand, `r` target, selection kept, snapshot round-trip and an older file, org switch, the query's shape and a parse with another owner.
- Live: with an empty cache, started inside `~/pgmac/terraform-pgpve` (only that repo fetched), `#32` showed `○ homelabia#221` and `○ homelabia#205` under it, which before #261 showed nothing.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (1277 pass).

## Deviations from the plan

- Batches ask for every **loaded** parent the refresh covers, not only visible ones, so a filter change shows children at once instead of after the next refresh.
- `r` on an outside row refreshes the group it is drawn in (as on a stub) instead of the issue itself: it has nothing loaded to patch, and the group refresh refetches it through its parent.
