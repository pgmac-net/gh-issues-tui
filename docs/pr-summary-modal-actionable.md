# PR summary modal actionable (#58, 2026-07-24)

Ticket: [pgmac-net/gh-issues-tui#58](https://github.com/pgmac-net/gh-issues-tui/issues/58) · PR: [#75](https://github.com/pgmac-net/gh-issues-tui/pull/75)

## What shipped

The PR summary popup (`Mode::PrSummary`) was read-only — no way to open the PR, jump to a failing check/run, or refresh CI status without closing and reopening it. The ticket asked for a brainstorming session to find a better way to present this information; the chosen fix (from a handful of options weighed with the user) was actionability, not a restructuring of the layout.

## Behaviour

- **`j`/`k`** still free-scroll the popup body, unchanged.
- **`Tab`/`Shift+Tab`** cycle a selection through the popup's open-able rows — the PR header, then each check, each PR workflow run, each default-branch run — wrapping at both ends and snapping the scroll to bring the selected row into view. The selected row is highlighted.
- **`o`/`Enter`** opens the selected row's URL in the browser (same `open::that` pattern as the list view's `o`).
- **`r`** re-fetches the summary in place, resetting to the loading state.
- Title bar updated to `PR summary (j/k scroll · Tab select · o open · r refresh · Esc close)`.
- **`PageUp`/`PageDown`** scroll by one viewport height, the same step `detail_page_rows` uses for the detail pane.
- **`Home`/`g`** jump to the top of the popup; **`End`/`G`** jump to the row past which only blank space remains. Both move the viewport only — the `Tab` selection (and so the URL `o`/`Enter` would open) is untouched.
- A vertical scrollbar is drawn on the popup's right edge whenever its content overflows the viewport (#105) — the same `render_region_scrollbar` widget the comments pane already uses, and a no-op when content fits.

## Implementation

| Area | File | Change |
|---|---|---|
| URLs on check/run data | `src/provider/types.rs` | `CheckContextInfo`/`WorkflowRunInfo` gain a `url` field; `PrRef::url()` derives the PR's own URL |
| GraphQL fetch | `src/github/client.rs` | `detailsUrl`/`targetUrl`/`url` added to the existing PR-summary query and DTOs — one round-trip, no new fetch |
| Selection model | `src/tui/app.rs` | `pr_sel` field; `App::pr_targets()` builds the ordered open-able rows with line offsets; `select_pr_target`/`pr_selected_url`/`refresh_pr_summary` |
| Key handling | `src/tui/event.rs` | `Tab`/`BackTab`/`o`/`Enter`/`r` in `handle_pr_summary_key` (now takes `client`/`tx` for the refresh re-fetch) |
| Rendering | `src/tui/ui.rs` | selected-row highlight (background patched onto each span, preserving fg/modifiers); updated title |

`App::pr_targets()`'s line offsets mirror `draw_pr_summary_popup`'s line layout by hand — the same convention `detail_metrics` already uses for the detail pane's split arithmetic. This is pure state logic, so it's unit-tested directly: target ordering/line offsets, `Tab` wrap-around with scroll-snap, selected-URL resolution, and `pr_sel` reset on open/close/refresh.

### Paging, Home/End and the scrollbar (#105)

| Area | File | Change |
|---|---|---|
| Bounded jumps | `src/tui/app/pr.rs` | `PrState::scroll_to_top`/`scroll_to_bottom(max)` — geometry-free, like the existing `scroll_by`/`clamp_scroll` |
| Page step | `src/tui/event/keys/shared.rs` | `pr_page_rows()` reads the live terminal via `ui::pr_summary_inner_height`, beside `pr_scroll_max` |
| Key handling | `src/tui/event/keys/pr.rs` | `PageUp`/`PageDown` (`scroll_by(±page, max)`), `Home`/`g` (`scroll_to_top`), `End`/`G` (`scroll_to_bottom(max)`) |
| Scrollbar | `src/tui/ui/pr.rs` | `draw_pr_summary_popup` calls `render_region_scrollbar` with the row count captured before the lines move into the `Paragraph` |

Home/End deliberately don't touch `pr.sel`: keeping the viewport and the `Tab` selection on separate axes means jumping to the top or bottom of a long popup can never silently change which row `o`/Enter would open next.

## Process & decisions

- Followed the `pickup-ticket` workflow's brainstorming path, since the ticket explicitly asked for one: the user picked "can't act on it" as the core pain (over wall-of-text, hard-to-read checks, or no in-modal navigation), then which actions mattered (open PR, open a failing check/run, refresh).
- Selection-vs-scroll model was presented as three options (Tab-cycles, j/k-moves-selection, only-failing-shortcut). The user's own answer diverged from all three: mirror the detail pane's established `j/k` scroll + `Tab`/`Shift+Tab` select convention exactly, for interaction consistency across the app — adopted as-is over the proposed alternatives.
- Plan rated STANDARD — implemented on Sonnet 5 (confirmed model switch before implementation).

## Verification

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` all clean — 276 tests, 4 new. Live headless smoke test (pty + pyte) against this repo's own linked PR (`#48`, which carries 6 real status checks): the fetch succeeded against the live GitHub API with the new `detailsUrl`/`targetUrl`/`url` query fields — the highest-risk part of the change, since it wasn't exercisable by the synthetic unit-test fixtures alone — and the popup rendered with the updated title bar. The pyte screen-capture tool itself hit an unrelated library-level crash scrolling deeper into the popup; confirmed via the raw pty byte log that this was a harness limitation, not a Rust panic (the app kept running throughout).

### #105 follow-up

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` clean — 390 tests, 7 new (2 in `app/pr.rs` covering paging clamp/top/bottom, 5 golden renders in `ui/pr.rs` including scrollbar-present/absent). No live smoke test needed — pure key-handling and rendering, no new fetch or query surface.
