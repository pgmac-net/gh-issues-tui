# 9. The list nests sub-issues, and keeps hidden parents as context

## Status

Accepted (#234).

## Context

#234 asks to show sub-issues in the list as foldable children under their parent, to any depth. GitHub links issues in three ways: sub-issues (one parent each), dependencies (blocked by / blocking) and related. Filters, scoped fetches and the open-only default can each hide a parent while its children still match. Children can also live in another repo, or even another owner.

## Decision

1. **The tree nests sub-issues only.** They are the only relationship with one parent per issue, so nesting is unambiguous and can't loop. Dependencies and related links are graphs; the detail pane counts them instead.
2. **A hidden parent stays as a *context row*.** When a parent is filtered out, unloaded, or in another repo, it is still drawn, dim, above its children. This keeps the tree's shape, and clearing a filter doesn't reshuffle the list. Context rows are never counted.
3. **The tree is built from each issue's ancestry, fetched with it.** GitHub's nested `parent` objects (to its 8-level limit) cost nothing in the bulk query (measured), so no extra requests are made and a context row can always be drawn.
4. **An unloaded context row is a read-only *stub*.** It is known only from a descendant's ancestry, so there is nothing loaded to write to: the write keys say so, and `o` opens it. A loaded context row behaves as any issue.
5. **Every issue stays in its own repo's group.** A parent from another repo is a context row in the child's group and a normal row in its own. Repo filters, header counts and `n` are unchanged.
6. **Pane keys follow ADR 0006.** `→` expands a collapsed parent, else opens the detail pane. `←` collapses an expanded parent, else goes to the parent row, else to the repo header. Collapse is keyed by issue id.

## Consequences

- A repo group can show rows that no filter matched. Code that walks `App.rows` for issues must use `Row::Issue`, or `App::row_issue` / `row_stub` for context rows. It must not assume every non-header row is a visible issue.
- Linear gives one level of parent (its children are a connection, which counts against its complexity budget) and Jira gives `fields.parent`. Deeper ancestors there show the nearest level only.
- Full lists of sub-issues, dependencies and related issues in the detail pane are a separate design. Today it shows a one-line summary.
