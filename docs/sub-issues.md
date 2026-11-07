# Sub-issues in the list (#234)

Sub-issues nest under their parent in the issue list, to any depth. A parent the filters hide stays as a dim context row, and the detail pane summarises an issue's links. Decision record: [ADR 0009](adr/0009-the-list-nests-sub-issues-and-keeps-hidden-parents-as-context.md). Terms: [CONTEXT.md](../CONTEXT.md#sub-issues-234).

## What you see

```
▾ ● #227  Refactor after organic growth  5/5  @pgmac [enhancement]
    ● #233  UI interaction consistency audit (#227 part 5, …)
    ● #232  Code reuse pass (#227 part 4, …)
```

- `▾`/`▸` on a parent; `5/5` is GitHub's own sub-issue progress, so it counts children that are closed, filtered out or in another repo.
- With a filter that hides the parent, the parent stays as a **context row**: hollow dot `○`, dim, no labels, assignees or dates.

  ```
  ▾ ○ #227  Refactor after organic growth  5/5
      ● #229  Granular refresh: issue / repo / filtered scope …
  ```

- A parent that isn't loaded at all (closed while showing open issues, outside a scoped fetch, or in another repo or owner) is drawn the same way from the child's ancestry. That is a **stub**: read-only, so the write keys (`x e c a l t p m A`) say it isn't loaded, and `o` opens it. Its detail pane shows its title, state and repo.
- Every issue stays in its own repo's group. A parent from another repo shows with its repo name (`api#12`) in the child's group, and as a normal row in its own.
- **Cross-repo sub-issues (#260, [ADR 0012](adr/0012-cross-repo-sub-issues-repeat-dim-under-their-parent.md)).** The reverse also holds: a parent lists its loaded sub-issues from other repos under it, as dim context rows, to any depth.

  ```
  api
  ▾ ● #10  Epic
      ○ web#20  Child in another repo
  web
    ○ api#10  Epic
        ● #20  Child in another repo
  ```

  - Only **loaded** sub-issues show; others (another owner, outside the loaded scope) are [#261](https://github.com/pgmac-net/gh-issues-tui/issues/261).
  - A child must pass every filter except the repo filter, so `terraform-pgpve` filtered to itself still shows `homelabia#221` under `#32`.
  - Only a visible issue of the group holds them: a context row never pulls in children, a filtered-out parent shows none, and a group with nothing visible is not brought back.
  - They sort with the parent's own children, by the current sort.
  - `Enter`/`→`/`Space` on a dim row from another repo jumps to its home row (clearing filters that hide it). On a collapsed parent it expands first.
- Context rows are never counted in the repo header or the list title.

### Detail pane

One line under the assignees/labels line, only when the issue has links, each part only when present:

```
parent: #227 Refactor after organic growth
parent: api#12 Refactor · sub-issues: 2/5 done · blocked by 1 · related 3
```

Full lists of sub-issues, dependencies and related issues are a follow-up design.

## Keys (ADR 0006's tree table)

| Key | On a parent | On a sub-issue | On a top-level issue |
|---|---|---|---|
| `→` / `Space` / `Enter` | collapsed: expand; expanded: open the detail pane | open the detail pane | open the detail pane |
| (same keys, on a dim row from another repo) | collapsed: expand; else jump to its home row | jump to its home row | n/a |
| `←` | expanded: collapse | go to its parent | go to the repo header |
| `[` / `]` | collapse / expand every repo group and every parent |||

Collapse is keyed by issue id: it survives refreshes and filter changes, and is cleared when you switch org. Sub-issue parents start expanded. Jumping with `#` to a hidden child expands its parents.

## Data

`Issue` carries `ancestors` (nearest first), `sub_issues` (completed/total) and `relations` (blocked by, blocking, related).

| Backend | Ancestry | Progress | Relations |
|---|---|---|---|
| GitHub | 8 levels (its limit) | `subIssuesSummary` | `issueDependenciesSummary`, `relatesTo { totalCount }` |
| Linear | 1 level (`parent`) | — | — |
| Jira | 1 level (`fields.parent`) | — | — |

**Cost.** Measured against pgmac-net, all of GitHub's fields cost nothing extra in the bulk query: nested `parent` objects at any depth, the two summaries, and `relatesTo { totalCount }`. A full org fetch costs 6 points before and after. Adding `first:` to a connection is what costs: `relatesTo(first: 1) { totalCount }` alone took the query from 1 point to 101. `issue_fields_has_no_nested_connections` still guards that.

Linear's children are only a connection, which would count against its per-query complexity budget on every issue, so it reports no progress and one level of parent. Neither Linear nor Jira has been checked live.

## How

- `app/subtree.rs` builds each group's forest. Visible issues and their ancestors are linked by `ancestors`, ordered by the current sort (an ancestor sorts where its first visible descendant does), and collapsed nodes' descendants are left out.
- `Row::Issue` gained `depth` and `branch`. `Row::Context` holds a `ContextRef`: `Loaded` (an issue anywhere in `App.repos`) or `Stub` (the child and the level in its ancestry).
- While building, visible issues are keyed by position and ancestors by id, so repeated ids can never merge rows.
- Cross-repo sub-issues (#260): `rebuild_rows` computes the passing issues of every repo once (`passing`), `cross_repo_links` finds, per group, the other repos' passing issues that hang under one of its passing issues (each at its nearest ancestor there), and `global_rank` puts every issue on one sort scale so they interleave. `group_rows` adds each as a context node keyed by id, linked up to its anchor.
- `selected_issue()` covers loaded context rows. `selected_repo()` is the group a row is drawn in; for an issue-scoped key use `selected_issue_repo()`, which is the issue's own repo. `selected_stub()` covers stubs, and one guard at the top of `handle_normal_key` makes stubs read-only.

## Plan deviations

- **The Related count stayed in.** The plan dropped it on an unmeasured cost assumption. Measured, an unpaginated count is free.
- **Linear gets one level of parent and no progress**, not a full chain with progress (complexity budget, no credentials to check).

## Live verification (pgmac-net, tmux)

- State "all": `▾ ● #227 … 5/5` with #229–#233 nested under it.
- `/Granular`: `▾ ○ #227 … 5/5` (context row), with #229 under it.
- Detail of #229: `parent: #227 Refactor after organic growth`. Detail of #227: `sub-issues: 5/5 done`.
- Full org fetch: 204 issues, 6 points, the same as before.

Not seen live: a stub (pgmac-net has no open sub-issue under an unloaded parent), cross-repo parents (none exist), and Linear/Jira. These are covered by unit tests.
