# Markdown in the PR summary popup (#102, 2026-07-30)

Ticket: [pgmac-net/gh-issues-tui#102](https://github.com/pgmac-net/gh-issues-tui/issues/102)

## What changed

The PR summary popup (`P`, `Mode::PrSummary`) rendered the PR description as raw
text — one `Line::raw` per source line. `## Heading` showed its hashes,
`**bold**` showed its asterisks, and a GFM table was a wall of pipes. The body
now goes through the same renderer the detail pane uses
([`markdown-rendering-detail-pane.md`](markdown-rendering-detail-pane.md) #67,
[`markdown-tables.md`](markdown-tables.md) #99), so the popup gained headings,
inline styles, fenced code, lists, quotes and tables in one move.

Body URLs are now OSC 8 clickable, the same terminal-native mechanism the detail
pane uses ([`clickable-urls.md`](clickable-urls.md), #80). They are deliberately
**not** `Tab` targets: `Tab`/`Shift+Tab` and `o` still cycle the PR header, each
check, and each workflow run, exactly as #58 defined them. A body line can carry
several links, and `PrRow` holds at most one URL, so making body links selectable
would have meant picking one arbitrarily.

`j`/`k` are now bounded. The popup's scroll had no upper limit at all — only
`u16::MAX` stopped it, so holding `j` scrolled into blank space. Markdown tables
expand one source row into several drawn rows, which made that much easier to
hit.

## Implementation

| Area | File | Change |
|---|---|---|
| Body rendering | `src/tui/ui/pr.rs` | `pr_summary_logical_rows` takes `width` and returns `(rows, Vec<LinkSpan>)`; the body loop becomes one `markdown::render_with_links` call |
| Link carriage | `src/tui/ui/pr.rs` | `pr_summary_rows` returns `(Vec<PrRow>, Vec<LinkRect>)`; `draw_pr_summary_popup` calls `apply_hyperlinks` after the `Paragraph` draws |
| Scroll bound | `src/tui/ui/pr.rs` | `pr_summary_inner_height` (split out of `pr_summary_area`) and `pr_max_scroll`, measured through the row model the popup draws |
| Clamp plumbing | `src/tui/event/keys/shared.rs`, `keys/pr.rs` | `pr_scroll_max` reads both terminal dimensions; `j` and both `Tab` arms apply it |
| Scroll arithmetic | `src/tui/app/pr.rs` | `PrState::scroll_by(delta, max)` and `clamp_scroll(max)` — the bound is passed in, since `app/` computes no geometry |

Two index rebases carry the whole change, and both mirror what
`markdown::render_with_links` already does internally:

- **Into the row list.** `LinkSpan::line` indexes the *output* line, not the
  source line, so the body's spans are offset by `rows.len()` at splice time. A
  table above a link expands rows underneath it; without this every hyperlink
  below the first table would land on the wrong cells.
- **Out through the wrapper.** `pr_summary_rows` wraps each logical line on its
  own — that is what pins "only the first wrapped row carries the URL" — so a
  line's spans are renumbered to line 0 going in, and the returned rects have
  `vrow` offset back out. `LinkRect::id` is remapped to the link's index in the
  whole popup, because ids are what a terminal uses to group a wrapped link into
  one hyperlink; leaving them per-line would merge two different URLs.

`pr_targets` and `pr_max_scroll` both measure with `Theme::default()`, the
convention `body_content_height` established — styling changes colour, never row
count.

## Testing

- 7 new tests in `ui::pr`: table body drawn as a table, heading/bold markup
  stripped, link rects indexed against their drawn row with unique ids, a link
  below a table indexed against the expanded output, `pr_max_scroll`'s two
  cases, and a golden that renders the popup scrolled *to* the bound and
  asserts the row model's last row is still on screen. That last one was
  verified to fail when the bound is off by one, so it cannot pass trivially.
- 3 new tests in `app::pr` pinning the clamp: `j` stops at the bound rather than
  running to `u16::MAX`, `k` does not underflow, and `clamp_scroll` never
  scrolls further down.
- The five pre-existing `golden_*` popup tests were left untouched on purpose.
  They scrape target rows out of the rendered buffer, so they are the regression
  signal for the row model: if the rebases were wrong, they fail.
- Full suite 370 → 384. `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean.
- Live-verified with the pty + pyte recipe in `.claude/skills/verify/SKILL.md`,
  against this repo's own PR #100 (whose description contains both a fenced
  pre-rendered example and a real 3-column GFM table). The real table rendered
  with `───┼───` rules and wrapped cells, `### Decisions` drew without its
  hashes, `**wrap**` without its asterisks, and the fenced block passed through
  untouched. Holding `j` (300 presses) came to rest with the last content row
  against the popup's bottom border rather than scrolling into blank space, and
  `Shift+Tab` to the last target landed on the same view.
- pyte's `Screen.display` crashes on that PR's `🤖` footer — it trips over the
  empty continuation cell ratatui writes after a wide glyph. Confirmed
  pre-existing by reproducing it on an unmodified build at the same scroll
  offset; the same harness limitation is recorded in
  [`pr-summary-modal-actionable.md`](pr-summary-modal-actionable.md).
