# Viewport scrolling (#189)

The issue list, the new-issue description box and the inline comment/body
editor all scroll with **minimal-scroll** semantics: moving the cursor inside
the visible window never moves the window; it scrolls only when the cursor
leaves it (selection reaches the top edge going up, the bottom edge going down).

## Why it was wrong

Each frame recomputed the viewport from nothing — `ListState::default()` for the
list, `cur_row - (height - 1)` for the editors — so the cursor was always pinned
to the *last* visible row and moving up scrolled at once.

## How it works

- `layout::scroll_top(prev, cursor, height, len)` is the one pure rule.
- The previous top is remembered in a `Cell<usize>` because `ui::draw` takes
  `&App`: `App.list_top` for the list, `BodyEditor.top` for both editors. The
  renderer writes it back after clamping.
- The list passes it to ratatui via `ListState::with_offset`.
- The top is clamped to `len - height`, so a shrunken terminal or shortened
  buffer never leaves the window past the content.
