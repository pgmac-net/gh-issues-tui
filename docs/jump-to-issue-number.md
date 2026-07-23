# Jump to issue by number (`#`)

Ticket: [pgmac-net/gh-issues-tui#62](https://github.com/pgmac-net/gh-issues-tui/issues/62)

## What it does

Press `#`, type an issue number, press `Enter` — the selector bar **moves to that
issue**. Unlike `/` free-text search (which narrows the list via the text filter),
`#` never filters the list down. It is a navigation aid, not a filter.

## Behaviour

- **Trigger** — `#` (previously unbound) opens a single-line input prompt,
  `issue # (Enter jumps)`. The value may be typed with or without a leading `#`.
  A non-numeric value shows `not an issue number` and does nothing.
- **Search order** — issue numbers repeat across repos (every repo has its own
  `#1`), so the jump searches the **currently-selected repo group first**, then the
  remaining groups in order. In the common single-repo / filtered view this is
  unambiguous; when browsing many repos it prefers the group you are already in.
- **Reveal, don't narrow** — if the target issue is loaded but currently hidden by
  the active filters or state filter, the jump **clears the filters and relaxes the
  state filter to `All`**, then selects it. This widens the visible list rather than
  narrowing it, honouring the ticket's "mustn't filter the list" intent. A collapsed
  repo group is expanded so the row exists.
- **Not loaded** — closed issues are only fetched after `f` (state filter) or
  `--all`. A number that is not in the loaded data shows `no issue #N loaded` and
  leaves the selection untouched; it does not fire a background refetch.
- **Detail pane follows** — the jump goes through the same `nav()` wrapper as
  `j`/`k` navigation, so an open, focused detail pane re-loads the jumped issue's
  comment thread.

## Implementation

| Area | File | Change |
|------|------|--------|
| Input kind | `src/tui/app.rs` | new `InputKind::GotoNumber` variant |
| Core logic | `src/tui/app.rs` | `App::jump_to_number(number: u64) -> bool` — current-repo-first rotation over loaded data, reveal-if-hidden (`clear_filters` + `state_filter = All`), expand collapsed group, relocate `selected` |
| Key binding | `src/tui/event.rs` | `#` in the Normal-mode handler opens the input |
| Submit | `src/tui/event.rs` | `GotoNumber` arm in `submit_input` parses the value and calls `nav(.., jump_to_number)` |
| Prompt label | `src/tui/ui.rs` | `input_prompt` → `issue # (Enter jumps)` |
| Help popup | `src/tui/ui.rs` | `("#", "jump to issue number")` |

`jump_to_number` is pure state logic (no I/O), so it is covered directly by unit
tests in `src/tui/app.rs`: match in the current repo, expansion of a collapsed
group, an absent number, current-repo-first collision resolution, and revealing a
filtered-out issue.

## Design decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Trigger key | `#` | Unbound and semantically "number"; leaves `/` as search |
| Collision (same number, multiple repos) | current repo group first | Predictable in the common filtered/single-repo view |
| Target hidden by a filter | clear filters + state → `All`, then jump | Ticket says never filter the list; clearing widens, it never narrows |
| Target not in loaded data | status message, no refetch | Keeps the jump synchronous and side-effect-free; closed issues load via `f`/`--all` |
