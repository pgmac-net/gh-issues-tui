# Detail pane: fixed body + per-comment scroll

Redesign of the issue detail pane's navigation, tracking
[gh-issues-tui#59](https://github.com/pgmac-net/gh-issues-tui/issues/59)
(follow-up feedback on the original PR #70).

## Motivation

The first detail pane (PR #70) used a **card cursor**: the body and every
comment were concatenated into one scrolling `Paragraph`, and `j`/`k` jumped
the scroll offset from one card's top to the next. For comments taller than
the pane this was hard to follow — you couldn't read *through* a long comment,
only jump past it, and the issue description scrolled away with everything
else. The card-offset arithmetic also didn't model wrapping, so the jump
target was only a lower bound.

The feedback asked for:

1. the description/body **pinned** at the top of the pane;
2. comments that **scroll**, with the comment header still selectable but the
   comment text scrollable too (press ↓ on a header to move into the text);
3. `Tab` / `Shift+Tab` to move between comments;
4. **scrollbars** wherever text is scrollable, showing position through the
   full text.

## Design

The pane splits **vertically into two independently scrolling regions**
(`ui::draw_detail` → `draw_detail_body` + `draw_detail_comments`):

- **Body region** (top, ≈45% — `detail_split`, `DETAIL_BODY_PCT`): issue
  metadata (number, title, state, author, dates, assignees, labels) + the
  rendered description, scrolled by `body_scroll`. Pinned — it never scrolls
  away with the comments.
- **Comments region** (bottom): the comment thread as stacked **cards** (an
  author · timestamp header rule, the body, a bottom rule), scrolled by
  `comments_scroll`.

Each region draws a `Scrollbar` on its right edge (`render_region_scrollbar`)
only when its content overflows the region.

### Selection and scrolling

Selection is `DetailSel { Body, Comment(usize) }` (`detail_sel` on `App`):

- `Tab` / `Shift+Tab` (`select_detail`) cycle `Body → Comment(0) → … →
  Comment(n-1)` and wrap. `←` returns focus to the list; `Esc`/`q` close the
  pane.
- `j`/`k` (and `↑`/`↓`) scroll the **selected** region only (`detail_scroll`
  in `event.rs` dispatches to `App::scroll_body` / `App::scroll_comment`);
  `PageUp`/`PageDown` step by the region's viewport height.
- Selecting a comment **snaps** its header to the top of the comments region
  (`snap_comment` + `comment_offset`). `comments_scroll` is an absolute offset
  into the stacked comments paragraph; scrolling then moves it within that one
  comment's extent, `[offset, offset + height − viewport]` (floored at
  `offset`, so a comment that fits doesn't scroll). The comments scrollbar
  therefore reflects position **within the selected comment**, not the whole
  thread.
- `clamp_detail_sel` keeps the selection valid when a shorter comment thread
  lands after a refetch (a past-the-end comment falls back to the last one; an
  empty thread falls back to the body).

### Accurate wrapped-height measurement

Scroll clamps need to know how tall the wrapped content is. Rather than
re-deriving word-wrap, the code enables ratatui's
`unstable-rendered-line-info` feature and calls `Paragraph::line_count(width)`,
which returns exactly the number of visual rows `Paragraph` will draw. This
removes the "wrapping not modelled, offset is a lower bound" caveat of the old
card model.

`ui.rs` builds each region's content **once** via shared builders — `body_lines`
and `comment_card_lines`. The renderer draws them with the real theme; the
measurement helpers (`body_content_height`, `comment_height`, `comment_offset`)
count them with a default theme (styling doesn't affect wrapping). The key
handler recovers the live region widths and viewport heights from the terminal
size in `detail_metrics`, mirroring `ui::draw`'s 40/60 horizontal split and
`detail_split`'s vertical split.

### Editing

Unchanged in spirit: `e` opens the one `Mode::CommentEditor` widget, now keyed
off `detail_sel` — the issue **description** when the body is selected, the
**comment** otherwise. While the editor is open it takes the bottom third of
the pane, leaving the body region visible above it.

## Key changes

| File | Change |
|------|--------|
| `Cargo.toml` | Enable ratatui `unstable-rendered-line-info` feature. |
| `tui/app.rs` | `DetailSel` enum + `detail_sel` / `body_scroll` / `comments_scroll` state (replacing `detail_card` / `detail_scroll`); `select_detail`, `scroll_body`, `scroll_comment`, `snap_comment`, `clamp_detail_sel`, `reset_detail_scroll`; `detail_split` layout helper. Removed `detail_card_count` / `move_detail_card` / `detail_card_offset` / `comment_card_lines`. |
| `tui/ui.rs` | `draw_detail` split into `draw_detail_body` + `draw_detail_comments` with per-region scrollbars; shared `body_lines` / `comment_card_lines` builders; `paragraph_height` / `body_content_height` / `comment_height` / `comment_offset` measurement helpers. |
| `tui/event.rs` | `Tab`/`Shift+Tab` move between comments in the pane; `j/k`/Page keys scroll the selected region via `detail_scroll` / `detail_page_rows`; `detail_metrics` / `snap_after_select` helpers. |
| `tui/markdown.rs` | Doc comment updated — the one-line-per-source-line property is no longer load-bearing for scroll sync. |

## Testing

- `app.rs`: `select_detail` cycle + wrap (and empty-thread no-op); `scroll_body`
  / `scroll_comment` clamps (including a comment that fits → no scroll);
  `clamp_detail_sel` fallback; `reset_detail_scroll`; `detail_split`; edit-target
  derivation from `detail_sel`.
- `ui.rs`: `body_content_height` / `comment_height` (incl. a wrapped line) /
  `comment_offset` against known geometry; a `TestBackend` render asserting both
  region titles, the pinned body, the selected comment header, and that
  scrollbar thumbs are drawn when content overflows.
- Full suite: 266 tests, `cargo clippy --all-targets -- -D warnings` clean,
  `cargo fmt --check` clean.

## Notes / deviations

- **Body is part of the Tab cycle.** So the body region stays reachable for
  scrolling (it has its own scrollbar). `Tab` wraps `Body → comments → Body`.
- **Split ratio** fixed at 45% body / 55% comments after checking a rendered
  `TestBackend` dump at 100×32 — the body gets its metadata plus ~7 description
  lines, comments get the majority. A tiny pane (`detail_split` ≤ 9 rows)
  collapses to body-only.
- Implemented on **Opus 4.8** though the plan rated the work COMPLEX (Fable 5);
  the model was kept per the same call as PR #70. No scope change.
