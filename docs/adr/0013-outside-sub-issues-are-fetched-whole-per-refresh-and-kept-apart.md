# 13. Outside sub-issues are fetched whole, per refresh, and kept apart

## Status

Accepted (#261). Extends ADR 0012, which showed only loaded sub-issues.

## Context

ADR 0012 draws a parent's sub-issues from other repos, but only those already loaded. A sub-issue in another owner, a repo not fetched (starting inside a clone fetches one repo), or outside a pushed-down filter (closed, another assignee) is missing. The bulk query cannot carry a `subIssues` connection (billed per issue slot, `docs/graphql-api-cost.md`), but every issue's `sub_issues.total` is free, so the app knows which parents are short of children.

## Decision

1. **Fetch them as full issues, not stubs.** The same fields as the list fetch, labels and assignees hydrated, so the filters apply to them exactly as to a loaded child (ADR 0012 clause 2). A stub would leak through every filter but state.
2. **Eagerly, in one batch, after each load or refresh that covers the parent.** A batch holds the loaded parents the refresh covers whose `sub_issues.total` exceeds the sub-issues loaded for them, through `nodes(ids:) { subIssues }` (about `1 + parents × 50/100` points; measured 1 for one parent). Loaded, not only visible: a filter change then shows them at once instead of waiting for the next refresh. The answer replaces those parents' outside sub-issues; until it lands the previous ones stay drawn. Mutations do not trigger it.
3. **Down the tree.** A fetched child that is itself short of children joins the next batch, level by level, until a level finds nothing or GitHub's 8-level limit.
4. **Kept apart from `App.repos`.** They are not in the loaded scope, so they never join a repo group, count, decide membership or feed the filter vocabulary as loaded data. They are context rows (`ContextRef::Outside`) and read-only: write keys say so, `o` and `Enter` open them in the browser, the detail pane shows them.
5. **In the snapshot.** Saved with it and drawn at startup (`serde(default)` for older files); they do not affect whether a snapshot is usable, and the startup refresh refetches them.
6. **GitHub only.** `Provider::sub_issues` defaults to unsupported; Linear and Jira are #263.

## Consequences

- A refresh can cost a few points more when parents are short of children, and nothing when none is.
- Code that walks issues for writes, counts or membership must keep reading `App.repos`; only the tree and the detail pane read `App.outside`.
