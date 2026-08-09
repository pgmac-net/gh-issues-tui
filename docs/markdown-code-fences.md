# Fenced code block rendering (#120, 2026-08-09)

Ticket: pgmac-net/gh-issues-tui#120

## What changed

Fenced code blocks (``` ``` ``` or `~~~`) in issue descriptions and comment
bodies now render as a distinct code block instead of showing the raw fence
delimiters:

```
▏ fn compute_total(items: &[Item], discount: f64) -> f64 {              rust
▏     items.iter().map(|i| i.price).sum::<f64>() * (1.0 - discount)
▏ }
```

(the gutter bar and `rust` tag are dim; the code itself sits on a filled
background — not visible in plain text above.)

Syntax highlighting was explicitly out of scope for the original renderer
([`markdown-rendering-detail-pane.md`](markdown-rendering-detail-pane.md), #67)
and stays out of scope here too — see [Deferred](#deferred) below.

## Decisions

Eight decisions, all taken on the ticket before implementation:

| # | Decision | Chosen | Why |
|---|---|---|---|
| 1 | Visual style | Dim `▏ ` gutter bar **and** a filled `code_bg` background | Strongest separation from prose; the gutter alone reads as another blockquote |
| 2 | Overflow | Hard-break at the pane edge, never word-wrap | A broken code line must never look like a fresh statement, and no character may be lost |
| 3 | Theme | New `code_bg`/`code_fg`, both explicit RGB | Self-consistent on any terminal background; existing profiles keep working (every field optional) |
| 4 | Language tag | Right-aligned on the block's first code row | Costs no extra row; a dedicated header row was rejected as the common case |
| 5 | Tag/code collision | Code wins — tag silently dropped | Never lose a code character to make room for a tag |
| 6 | Syntax highlighting | Separate follow-up ticket, in-house (no `syntect`) | Keeps #120 reviewable; avoids a heavy C/regex dependency across 4 release platforms |
| 7 | Indented code blocks | Out of scope — fenced only | The renderer has no nested-list state, so 4-space indentation can't be told apart from a wrapped list continuation |
| 8 | Inline `` `code` `` | Same `code_fg`/`code_bg` chip treatment | One visual language for "this is code," not two |

## Approach

`src/tui/markdown/fence.rs` mirrors `table.rs`'s block contract: `fence::parse`
consumes a whole source block (opening delimiter through the matching closer,
or EOF if unterminated) and `fence::render` **pre-wraps its own output**, so
`linkmap::wrap` never re-wraps it. That's what keeps the gutter bar and
background fill attached to every continuation row — `linkmap` treats an
already-wrapped block as a set of rows that each happen to already fit.

`mod.rs`'s dispatch loop now tries `fence::parse` before `table::parse`, so a
`|` inside a code fence is never mistaken for a table
(`a_table_inside_a_fence_is_not_parsed` in `table.rs` pins this both ways).

**Hard-break is grapheme-aware, not word-aware.** `fence::hard_break` chunks a
content line by display width using `unicode-segmentation` + `unicode-width` —
the same crates `linkmap` already depends on — breaking strictly at the width
boundary. This deliberately diverges from `linkmap`'s own break rule, which
seeks the last whitespace that fits; code must never be re-flowed at a fake
word boundary.

**The language tag competes with code for the first row's trailing space.**
`fence::render` computes `available = code_area - chunk_width` for the very
first rendered row only; the tag is drawn flush-right when
`available >= tag_width`, otherwise dropped. No other row is ever a tag
candidate.

**Rows are built as spans, not flat strings.** The hard-break splits a
`Vec<Span>`-shaped row (gutter, code, optional padding, optional tag), so a
future syntax highlighter can slot in more code spans without touching the
break, padding, or tag logic.

**Inline code reuses the same style function.** `code_style()` in `mod.rs`
changed from `fg(t.dim)` to `fg(t.code_fg).bg(t.code_bg)`, and both
`fence::render` and `inline.rs`'s `` ` `` handling call it — so the chip
treatment lands on inline and fenced code from one change.

### The invariant this gives up

Fences join tables (#99) as the second exception to "one output `Line` per
source line": both fence delimiter lines are dropped, and a long content line
can expand into several output rows. That's safe for the same reason tables
are — `body_content_height`/`comment_height` measure by rendering through the
*same* function at the *same* width the pane draws with, so measured and drawn
heights cannot disagree. `detail.rs`'s new
`fence_measured_height_matches_the_rendered_rows` pins this at three widths,
including one narrow enough to force a mid-line hard-break.

### Degenerate width

A pane narrower than the 2-cell gutter (`width <= 2`) falls back to flat,
unpadded `code_fg`/`code_bg` lines with no gutter and no hard-break, so an
absurdly narrow pane can't panic or emit a zero-width row.

## Deviations from the plan

None. The gutter-plus-background visual, the hard-break rule, the two-key theme
addition, the right-aligned/drop-on-collision tag policy, the fenced-only scope,
and the shared inline/block code style all landed exactly as decided on the
ticket.

## Testing

- 14 unit tests in `markdown::fence`: parsing (terminated, unterminated, `~~~`,
  non-fence), delimiter lines never surviving into output, the empty-block
  case, every row landing at exactly the requested width, hard-break losing no
  character, the tag drawn/dropped/absent cases, and the degenerate
  `width <= 2` fallback.
- 3 existing `markdown::tests` updated to the new contract: the two line-count
  invariant tests now subtract the dropped delimiter line(s), and
  `inline_code_span_is_code_styled` (renamed from `..._is_dim_styled`) asserts
  `code_fg`/`code_bg` instead of `t.dim`.
- `table::tests::a_table_inside_a_fence_is_not_parsed` updated: pipes inside a
  fence stay literal text under a gutter, rather than the old raw-fence-line
  expectation.
- 1 new integration test in `ui::detail`,
  `fence_measured_height_matches_the_rendered_rows`, alongside the table's
  existing measured-vs-drawn check.
- Full suite 421 passed. `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean.
- Live-verified against a real comment posted to
  `pgmac-net/gh-issues-tui#120` (a Rust fence and a short shell fence) using the
  pty + pyte recipe in `.claude/skills/verify/SKILL.md`: both blocks rendered
  with the gutter bar, no backticks anywhere, and their language tags
  (`rust`, `sh`) right-aligned on each block's first row. The fixture comment
  was deleted after capture — it was scratch verification data, not part of
  the ticket's discussion.

## Deferred

Filed as a follow-up: an in-house, dependency-free per-language token
highlighter (keywords/strings/comments/numbers), built directly on the
span-based row structure this change introduces so it can slot in as a second
pass over `Fence::content` rather than a rewrite of `fence::render`.
