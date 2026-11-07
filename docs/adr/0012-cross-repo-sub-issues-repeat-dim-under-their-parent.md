# 12. Cross-repo sub-issues repeat dim under their parent, and link home

## Status

Accepted (#260). Supersedes clause 5 of ADR 0009 for sub-issues, and amends
clause 6 for context rows from another repo.

## Context

ADR 0009 kept every issue in its own repo's group: a parent from another repo
was drawn as context above its child, but the child was not repeated under the
parent (pinned by `a_parent_in_another_repo_is_context_here_and_a_normal_row_at_home`).
So a parent such as `terraform-pgpve#32`, whose sub-issues all live in
`homelabia`, looked childless in its own group.

## Decision

1. **A visible parent shows its loaded sub-issues from other repos, dim, at
   their real depth (the full subtree).** They are context rows, never
   counted, and are derived from the descendants' ancestry like every other
   tree link, so no request is added. Unloaded ones (another owner, outside
   the loaded scope) need a per-parent fetch and are #261.
2. **They pass every filter but the repo filter.** A child hidden at home by
   state, assignee, author, label or text is hidden here too; a repo filter
   naming the parent's repo does not hide them, since that is the case the
   feature is for.
3. **Only visible issues anchor them.** A context row never pulls in
   descendants, so an epic drawn as context does not drag its tree into every
   group, a filtered-out parent shows no children, and `hide_empty` is never
   undone by context rows.
4. **They sort with same-repo children by the active sort.**
5. **A context row from another repo is a link home, and only `Enter`
   follows it.** `Enter` jumps to its home row (as `#N` jump does, widening
   filters if needed), for ancestors as well as descendants. `←`, `→` and
   `Space` act on the dim row itself, as on any row: collapse, expand, or open
   the detail pane. A context row in its own repo keeps ADR 0009's behaviour.
   (First built with `→`/`Space` jumping too; that took the selection away
   when you only meant to fold the tree.)
   The selection also stays on a dim row across a refresh, a patch and a
   collapse: it is re-found by issue id *and* the group it was drawn in
   (`RowKey`), since the id alone names the home row.
6. **Issue-scoped keys use the issue's home repo, not the group drawn in.**
   Labels, priority, copy ref, harness ref, PR shorthand and the runs tab
   resolve the repo from the selected issue; group-scoped keys (`n`, collapse,
   `r` on a header) keep the group.

## Consequences

- The same issue can now be drawn in several groups. Collapse is keyed by
  issue id, so collapsing it in one group collapses it in all.
- Code that needs "the repo of the selected issue" must not use
  `selected_repo()`, which is the group.
