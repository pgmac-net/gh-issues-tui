# PR modal scrolling (#105, 2026-07-31)

Ticket: [pgmac-net/gh-issues-tui#105](https://github.com/pgmac-net/gh-issues-tui/issues/105) · PR: [#106](https://github.com/pgmac-net/gh-issues-tui/pull/106)

## What shipped

The PR summary popup (`Mode::PrSummary`) only supported `j`/`k` free-scroll and `Tab`/`Shift+Tab` row selection — no PgUp/PgDn paging, no Home/End jump, and no visual indication of how far through the content the viewport was, unlike the comment thread in the detail pane.

## Behaviour

- **`PageUp`/`PageDown`** scroll by one viewport height — the same step `detail_page_rows` already uses for the detail pane.
- **`Home`/`g`** jump to the top; **`End`/`G`** jump to the row past which only blank space remains.
- Home/End move the viewport only — the `Tab` selection (and so the URL `o`/`Enter` would open) is left untouched. This was the sharpest decision in grilling: the two axes (scroll position, selected row) stay independent, so jumping to the top/bottom of a long popup can never silently change which check or run `o` would open next.
- A vertical scrollbar now draws on the popup's right edge whenever content overflows — a no-op when it fits.

## Implementation

| Area | File | Change |
|---|---|---|
| Bounded jumps | `src/tui/app/pr.rs` | `PrState::scroll_to_top()` / `scroll_to_bottom(max)`, geometry-free like the existing `scroll_by`/`clamp_scroll` |
| Page step | `src/tui/event/keys/shared.rs` | `pr_page_rows()` reads the live terminal via `ui::pr_summary_inner_height`, beside `pr_scroll_max` |
| Key handling | `src/tui/event/keys/pr.rs` | `PageUp`/`PageDown` → `scroll_by(±page, max)`; `Home`/`g` → `scroll_to_top()`; `End`/`G` → `scroll_to_bottom(max)` |
| Scrollbar | `src/tui/ui/pr.rs` | `draw_pr_summary_popup` calls `render_region_scrollbar` — the exact widget the comments pane already uses — with the row count captured before the lines move into the `Paragraph` |

Nothing here is new machinery: every piece (the scrollbar widget, the page-step convention, the bounded-scroll pattern) already existed elsewhere in `tui/` for the detail pane and comments thread, and was applied to the PR popup's existing row model (`pr_summary_rows`/`pr_max_scroll`/`pr_summary_inner_height`) without needing any new geometry.

## Process & decisions (grilling)

Four decisions were put to the user before planning, each with a recommendation:

1. **Home/End: scroll only, not also snap the `Tab` selection.** Chosen so the two axes stay independent — the alternative (snapping `sel` to the first/last target) would make Home/End silently change what `o`/Enter opens.
2. **Page step = full viewport height**, matching `detail_page_rows` rather than diverging with a viewport-minus-one "keep one line of context" step.
3. **Popup title hint left unchanged.** It already occupies ~71 of 74 available cells (`" PR summary (j/k scroll · Tab select · o open · r refresh · Esc close) "`); adding the new keys would truncate on narrow terminals. Widening the popup instead was rejected — it would touch every golden layout test pinning `popup.width == 76` for a hint string. The new keys behave conventionally and are documented here and in `docs/pr-summary-modal-actionable.md`.
4. **`g`/`G` added as Home/End aliases**, mirroring the list view's `g`/`G` → Home/End convention. Both were unbound in `Mode::PrSummary`, so no collision.

No ADR: the change introduces no new domain terms and no hard-to-reverse decision — it's the popup's existing conventions applied to four more keys.

Plan rated STANDARD — implemented on Sonnet 5 (session was already on Sonnet 5 at pickup, so no model-switch round trip was needed before implementation).

## Deviations from plan

One: the planned `PrState` method names `to_top()`/`to_bottom()` triggered `clippy::wrong_self_convention` (a `to_*` method taking `&mut self` rather than `self`/`&self` is a naming smell in Rust). Renamed to `scroll_to_top()`/`scroll_to_bottom()` before landing — same behaviour, clippy-clean name.

The plan also called for key-handler-level tests directly exercising `handle_pr_summary_key`. On checking the codebase, the sibling `PageUp`/`PageDown` wiring in `keys/normal.rs`/`keys/detail.rs` has no such tests either — only the underlying state methods and golden UI renders are tested. Followed that existing convention instead: coverage lives in `app/pr.rs` unit tests (`scroll_by`/`scroll_to_top`/`scroll_to_bottom`) and `ui/pr.rs` golden renders, not a new key-dispatch test layer.

## Verification

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` all clean — 390 tests, 7 new (5 in `app/pr.rs` covering the paging clamp and both jumps; 2 golden renders in `ui/pr.rs` for scrollbar-present and scrollbar-absent). One pre-existing golden test (`golden_scrolled_to_the_bound_the_last_row_is_still_drawn`) needed its border-trim character set widened from `['│', ' ']` to also strip `'║'`/`'█'` — ratatui's default vertical scrollbar track glyph is `║` (double vertical line) and the thumb is `█`, and that test scrolls to the popup's max bound, which now draws a scrollbar. No live/API smoke test needed — pure key-handling and rendering, no new fetch or query surface touched.
