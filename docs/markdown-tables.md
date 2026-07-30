# Markdown table rendering (#99, 2026-07-29)

Ticket: pgmac-net/gh-issues-tui#99

## What changed

GFM pipe tables in issue descriptions and comment bodies now render as tables
instead of raw pipes — aligned columns, a header rule, and `│` separators:

```
 Repo          │ Status │ Notes
───────────────┼────────┼────────────────────
 homelabia     │ open   │ needs a regression
               │        │ test
 gh-issues-tui │ closed │ shipped
```

Tables were explicitly out of scope for the original renderer
([`markdown-rendering-detail-pane.md`](markdown-rendering-detail-pane.md), #67).

Since #102 the PR summary popup shares this renderer too
([`pr-summary-markdown.md`](pr-summary-markdown.md)), so `Table::render` has a
second width consumer: the popup's fixed 74-cell inner width, independent of the
terminal size.

## Decisions

Seven decisions, all taken deliberately on the ticket before implementation:

| # | Decision | Chosen | Why |
|---|---|---|---|
| 1 | Visual style | Header rule + `│` separators, no outer box | Cheapest on width; the pane is only 46 cells at 80×24 |
| 2 | Overflow | Fit to width, **wrap** cells | Nothing is lost off-screen |
| 3 | Cell content | Full inline markdown, including clickable links | Consistent with #80 elsewhere in the pane |
| 4 | Detection | Strict GFM; ragged rows padded/truncated | Matches what github.com renders for the same body |
| 5 | Width policy | Fair-share water-fill | A short column is never crushed to feed a wide one |
| 6 | Alignment | Honour `:---`, `---:`, `:---:` | GFM parity |
| 7 | Degenerate pane | Floor 3 cells/column, never bail | Beats a stack of single characters |

## Approach

`src/tui/markdown.rs` was at 408 production lines — the ~400-line ceiling
`CLAUDE.md` sets for `tui/`. It became a directory, matching the existing
`app/` `event/` `ui/` split:

| File | Holds |
|---|---|
| `markdown/mod.rs` | `LinkSpan`, `render_with_links`, block dispatch, fence state |
| `markdown/inline.rs` | `parse_inline_links` and its helpers (moved verbatim) |
| `markdown/table.rs` | Detection, parsing, water-fill layout, row rendering |

`render_with_links` now takes the pane's inner text width. Both call sites in
`ui/detail.rs` already held that width; it was simply not plumbed down. Width is
consulted *only* for tables — every other block is still emitted unwrapped for
`linkmap` to wrap.

**Cell wrapping reuses `linkmap::wrap`** rather than reimplementing the break
rule. A cell is wrapped by calling the detail pane's own wrapper on a
single-line slice, which also returns the link rects — so table cells break
exactly like every other line in the pane, and cannot drift from it.

**Layout.** Natural column width is the widest *rendered* cell (so `**done**`
measures 4, not 8). The budget is `width - (1 + 3*(ncols-1))`. When natural
widths exceed it, a binary search finds the highest level `L` where
`sum(min(natural, L)) <= budget`; each column takes `min(natural, L)`, the
integer-division remainder goes to the capped columns, and every column floors
at 3 cells without ever being widened past its natural width.

### The invariant this gives up

The renderer's one-`Line`-per-source-line property no longer holds: a table row
occupies as many screen rows as its tallest cell. That is safe because
`body_content_height` and `comment_height` measure by rendering through the
*same* function at the *same* width the renderer draws with, so measured and
drawn heights cannot disagree. `detail.rs`'s
`table_measured_height_matches_the_rendered_rows` pins this at three widths.

### A bug this exposed

`markdown.rs` stamped each `LinkSpan` with the **source** line index. Output and
source indices were identical until now, so it was invisible; the moment a table
expands rows, every hyperlink below it lands on the wrong cells. Links are now
indexed against `out.len()` at push time, pinned by
`link_after_a_table_is_indexed_against_the_expanded_output`.

## Deviations from the plan

Two, both minor:

- The plan did not mention header styling. Header cells render **bold** (no
  colour change), consistent with how the renderer bolds headings.
- The plan said cells would be padded and aligned; in practice rows also need
  their trailing padding trimmed, including the dangling `" │ "` left by a row
  whose last cell is empty. Without it, a ragged row carried an invisible tail.

The module split, water-fill algorithm, `linkmap` reuse, link-index fix, and all
seven decisions landed as planned.

## Testing

- 12 unit tests in `markdown::table`: detection (including both negative cases),
  ragged rows, alignment, water-fill + wrapping, escaped `\|`, wide glyphs,
  fenced tables, the 10-column degenerate case, links in cells, and the
  link-index regression.
- 2 integration tests in `ui::detail` pinning measured height against drawn rows
  and against the table's laid-out row count. The latter was verified to fail
  when the table dispatch is disabled, so it cannot pass trivially.
- Full suite 360 → 374. `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean.
- Live-verified against `pgmac-net/gh-issues-tui#99`'s own comment thread (which
  contains a 3-column table) using the pty + pyte recipe in
  `.claude/skills/verify/SKILL.md`, at 200 and 100 columns. At 100 columns the
  `#` column kept its natural width of 1, `Decision` its 15, and `Chosen`
  absorbed the shrink and wrapped — with continuation rows correctly showing
  empty cells behind their separators.
