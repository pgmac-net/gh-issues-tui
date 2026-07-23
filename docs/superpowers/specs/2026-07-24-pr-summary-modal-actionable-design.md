# PR summary modal — make it actionable

**Ticket:** pgmac-net/gh-issues-tui#58
**Date:** 2026-07-24
**Status:** Approved design

## Problem

The PR summary popup (`Mode::PrSummary`, rendered by `draw_pr_summary_popup` in
`src/tui/ui.rs`) is a read-only wall of text: header, state/diffstat, the raw PR
body, reviews, comments, checks + contexts, PR workflow runs, and default-branch
runs — all one flat `Paragraph` scrolled with `j/k`. The chosen top pain is that
**it's a dead end**: you can't open the PR, jump to a failing check or run, or
refresh CI status without closing and reopening.

## Goal

Turn the modal into a navigable, actionable view while keeping its layout and
staying consistent with the detail pane's existing interaction model
(`j/k` scroll, `Tab` moves between selectable items).

## Design

### 1. Data — give checks and runs their URLs

Nothing in the summary is openable today because the check/run types carry no
URL. Add a URL field to each, populated in the **existing** GitHub GraphQL query
(`Client::pull_request`), honoring the "fetched in one GraphQL query" invariant:

- `CheckContextInfo { name, conclusion }` → add `url: String`.
  Source: `CheckRun.detailsUrl` for check runs, `StatusContext.targetUrl` for
  legacy statuses (the same `ContextNode` union already flattened in the DTO).
- `WorkflowRunInfo { workflow, run_number, event, conclusion, created_at }` →
  add `url: String`. Source: the workflow run's `url`.

The PR's own URL is derived from `PrRef`
(`https://github.com/{owner}/{repo}/pull/{number}`) — no fetch needed. A
`PrRef::url()` helper alongside the existing `PrRef::label()`.

**Approaches considered:**
- **(A) Fetch URLs in the existing query — chosen.** One round-trip, consistent
  with the current single-query design.
- (B) Construct URLs client-side from patterns — rejected. Run and check URLs
  aren't derivable from a run *number*; only the PR URL is.
- (C) Lazy-fetch a URL when a row is opened — rejected. Extra round-trip and
  extra in-flight state for no benefit.

### 2. Selection model — mirror the detail pane

Keep `j/k` as viewport scroll (`pr_scroll`, unchanged). Add `Tab` / `Shift+Tab`
to move a selection through the **targets** — the openable anchors — mirroring
`select_detail` and the comment-card snap already used in the detail pane.

Targets, in display order:

1. the PR itself (the header row) — so "open the PR" is the default: open the
   modal, press `o`.
2. each check context (in `checks.contexts` order)
3. each PR workflow run (`pr_runs`)
4. each default-branch run (`default_branch_runs`)

State on `App`: add `pr_sel: usize` (index into the target list) beside the
existing `pr_scroll`. Moving selection snaps the selected row into view by
adjusting `pr_scroll` (same approach as `snap_comment`/`comment_offset`). The
selected row renders with a highlight (reversed / `accent` style) in
`draw_pr_summary_popup`.

`pr_sel` resets to 0 (the PR header) on `open_pr_summary`, `close_pr_summary`,
and refresh.

To keep target construction testable and in one place, a pure helper on `App`
(e.g. `pr_targets() -> Vec<PrTarget>`) builds the ordered list from
`pr_summary`, where `PrTarget` carries the kind and its URL. Both the key
handler (for `o`/`Enter`) and the renderer (to know which display row is
selected) derive from the same helper.

### 3. Actions

In `handle_pr_summary_key`:

- `j` / `k` → `pr_scroll` ± 1 (unchanged)
- `Tab` / `Shift+Tab` → `pr_sel` next / previous target, clamped at the ends,
  snapping into view
- `o` / `Enter` → `open::that(url)` for the selected target, with the existing
  `opened {url}` / `open failed: {e}` status line (same pattern as the list-view
  `o` handler)
- `r` → refresh in place: set `pr_summary = None` (shows "loading…"), reset
  `pr_sel`/`pr_scroll`, and re-`spawn_pr_summary(pr_target)`
- `Esc` / `q` → close (unchanged)

Title bar: `PR summary (j/k scroll · Tab select · o open · r refresh · Esc close)`.

The `r` refresh needs the `Provider` and event `tx` in the handler; the summary
key handler gains those params (it currently takes only `app`/`key`), matching
`handle_pr_picker_key`.

### 4. Out of scope (YAGNI)

- No "wall of text" restructuring or collapsible sections — actionability was
  the chosen pain, and the selection highlight already adds structure.
- No copy-URL action (not requested).
- Body rendering unchanged.

## Testing

Pure-logic unit tests in `src/tui/app.rs` (the no-I/O module):

- `pr_targets()` builds the list in the right order and length from a
  `sample_pr_summary` (header + N contexts + N pr_runs + N default_branch_runs).
- Each target resolves to the correct URL (PR header → derived PR URL;
  check → context url; run → run url).
- `Tab`/`Shift+Tab` navigation clamps at both ends.
- `pr_sel` resets to 0 on open / close / refresh.

The `sample_pr_summary` fixture and any `CheckContextInfo` / `WorkflowRunInfo`
constructions gain the new `url` field.

Existing gates must stay green: `cargo clippy --all-targets -- -D warnings`,
`cargo test`, `cargo fmt --check`.

## Touched files

- `src/provider/types.rs` — `url` on `CheckContextInfo`/`WorkflowRunInfo`,
  `PrRef::url()`.
- `src/github/client.rs` — fetch the URLs in the PR-summary GraphQL query + DTO
  mapping.
- `src/tui/app.rs` — `pr_sel`, `pr_targets()`/`PrTarget`, reset points, tests.
- `src/tui/event.rs` — `Tab`/`o`/`Enter`/`r` in `handle_pr_summary_key`
  (+ `Provider`/`tx` params).
- `src/tui/ui.rs` — selected-row highlight, updated title.

## Complexity

STANDARD — implement on Sonnet.
