# Cross-repo sub-issues under their parent (#260)

A parent issue now lists its sub-issues from other repos, dim, under it, and selecting one jumps to the issue in its own repo. Decision record: [ADR 0012](adr/0012-cross-repo-sub-issues-repeat-dim-under-their-parent.md). Terms: *context row*, *home row*, *anchor* in [CONTEXT.md](../CONTEXT.md). User-facing behaviour: [sub-issues.md](sub-issues.md).

## Problem

Since #234 a sub-issue in another repo showed its parent as a dim row above it, but the parent's own group showed no children. `terraform-pgpve#32`, whose sub-issues `homelabia#221` and `#205` are all in `homelabia`, looked childless.

## Decisions (from the ticket's grilling)

| Question | Decision |
|---|---|
| Which children | Loaded ones only. No new request: the tree is built from each issue's ancestry, and the bulk query cannot carry a `subIssues` connection. Unloaded ones: #261. |
| Following a dim row from another repo | `Enter` alone jumps to its home row (children and ancestors alike). `←`/`→`/`Space` act on the dim row itself. |
| Filters | Every filter but the repo filter applies. |
| Depth | The full subtree, hidden middle levels as context. |
| Anchoring | Only visible issues of a group anchor children; a context row never does. |
| Order | Interleaved with same-repo children by the active sort. |
| Wrong-repo keys | Issue-scoped keys use the issue's home repo (`selected_issue_repo()`), group-scoped keys keep the group. |

## How

- `rebuild_rows` computes `passing` (issues passing every filter but the repo filter) for every repo once.
- `cross_repo_links` finds, per group, the other repos' passing issues that hang under one of its passing issues, at the nearest ancestor there.
- `global_rank` ranks all issues on one sort scale (`cmp_issues`, shared with `sort_issues`), so cross-repo and same-repo children interleave.
- `group_rows` adds each as a context node keyed by id and links it up to its anchor; ancestors in between merge with the existing nodes.
- `tree_right_node` jumps on a loaded context row from another repo, via `jump_to_ref`, which also clears any filter hiding the target.

## Review follow-up: Enter only, and the selection stays put

The first version also jumped on `→` and `Space` (on an expanded parent), and `←`/`→` re-selected the issue by id after a collapse or expand. An id names the issue's home row, so folding the tree on a dim row carried the selection to another repo. Now:

- `Enter` is the only key that follows a link. `←`, `→` and `Space` collapse, expand, or open the detail pane on the dim row itself.
- The selection is restored by a `RowKey` (issue id plus the group the row was drawn in), at every re-select: collapse, expand, refresh, patch, rank merge, created issue. A refresh no longer moves you off a dim row either.

Other keys: `o` opens the issue itself; write keys on a loaded dim row act on that issue (in its own repo); `#N` jumps to a home row; `n` creates in the group you are looking at; `O` opens the issue's own repo.

## Limitation, since lifted

This shipped with loaded sub-issues only: an exact repo filter narrows the fetch to that one repo (ADR 0008), so starting inside the parent's repo with nothing cached showed none from other repos. #261 lifted it: the missing ones are fetched for their parent as *outside sub-issues* ([outside-sub-issues.md](outside-sub-issues.md), ADR 0013).

## A bug this fixed

On a loaded context row from another repo, `selected_repo()` is the group the row is drawn in, not the issue's repo. Labels, priority, `y`, the harness ref, PR shorthand, the runs tab and move targets all used it, so on `terraform-pgpve#32` shown in `homelabia`, `l` loaded `homelabia`'s labels. They now use `selected_issue_repo()`.

## Verification

- Unit tests in `app/subtree.rs`: shape, counts, repo filter, other filters, no anchoring by context rows, filtered-out parent, full depth, hidden middle, interleave, collapse across groups, jump both ways, jump through a repo filter, same-repo context keeps detail, issue-scoped keys.
- Live (pgmac-net): started from `~/pgmac/terraform-pgpve` (exact repo filter), `#32` showed dim `homelabia#221` and `#205`; Enter on one landed on its row in `homelabia`.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (1253 pass).

## Deviations from the plan

None in behaviour. `selected_issue_repo()` reads the repo straight from the context row's `ContextRef::Loaded` instead of searching by id, which is cheaper and safe if ids repeat.
