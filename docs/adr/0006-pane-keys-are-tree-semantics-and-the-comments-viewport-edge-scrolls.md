# 6. Pane keys are tree semantics, and the comments viewport edge-scrolls

## Status

Accepted (#217). Reverses the per-comment scroll clamp and scrollbar from #59.

## Context

By v0.14.4 the same keys meant different things depending on which pane had
focus, and not in a consistent way:

- In the tree, `←`/`→` collapsed and expanded repo groups, and `→` on an issue
  moved into the detail pane. In the detail pane `←` always returned to the
  tree and `→` did nothing; folding a comment (#210) was only `Space`.
- `Space` on an issue row collapsed the issue's whole repo group, while `Enter`
  opened the issue. On a comment, `Space` folded and `Enter` did nothing.
- `↑`/`↓` moved tree rows, but in the detail pane they scrolled *within* the
  selected region. Moving to the next comment was `Tab` only, and selecting a
  comment snapped its header to the top of the viewport.
- `g`/`G` always moved the tree selection, even with the detail pane focused
  (`normal.rs` had no focus split for them).

#59 had clamped `comments_scroll` to the selected comment's own extent and
drawn the scrollbar over that comment alone. That was what made `↑`/`↓`
unable to cross from one comment into the next.

## Decision

1. **Every pane with nested or foldable content uses the tree's key
   semantics.** `↑`/`↓` move the selection, `←` goes out one level, `→` goes in
   one level, and `Space` and `Enter` are the same key: toggle a fold or
   collapse, or `→` on a leaf that has a next level (an issue). The full
   tables are in `docs/consistent-pane-keys.md`. Today that is the tree and
   the detail pane; a future nested pane must follow the same tables. Modals
   (Yes/No, help topics, calendar, pickers) keep their own keys, because none
   of them has anything to fold.
2. **Keys act on the focused pane only.** Each key with a meaning in both
   panes is a single match arm split on `app.focus`. The unfocused pane's
   selection is drawn dimmed (`selected_bg_unfocused`), so the eye goes to
   the pane the keys will act on.
3. **In the detail pane the selection is a whole item, moved by a row
   cursor.** `↓` on an item whose bottom is below the viewport scrolls one row.
   Once the bottom is visible, the next `↓` selects the next item. `↑` mirrors
   this. The sequence `Body → Comment(0) → … → Comment(n-1)` does not wrap.
   `Tab` still jumps header to header, and still wraps.
4. **The comments viewport moves only at an edge, by the minimum.** Selecting
   a comment no longer snaps its header to the top. A fold or unfold keeps the
   selected header on the screen row it was on. `comments_scroll` is clamped
   to `[0, thread − viewport]`, and the scrollbar shows position in the whole
   thread. This replaces #59's `scroll_comment(lo, hi)` and its per-comment
   scrollbar.
5. **`h`/`l` are not added as mirrors of `←`/`→`.** `l` stays "labels".

## Consequences

- One mental model across panes: you can walk a thread with the same keys you
  use to walk the tree, and back out of either with `←`.
- `Space` on an issue no longer collapses its repo. Collapsing from an issue
  is now `←` (to the header) followed by `←` (collapse).
- `Enter` on an issue goes through `enter_detail`, so in an already-open pane
  it keeps your place instead of resetting the scroll.
- The geometry the moves need (every comment's wrapped height) is measured
  by the key handler and passed into pure `DetailState` methods as a
  `DetailGeometry`. `app/` still computes no screen geometry itself. The
  minimal-scroll rule is one pure function, `layout::scroll_into_view`, and
  the renderer clamps to the same `[0, thread − viewport]` range, so a
  terminal resized between keys still draws a full viewport.
- Reading a single very long comment no longer shows "how far through this
  comment" on the scrollbar. The trade was judged worth it: position in the
  thread is the more useful signal once `↓` crosses comments.
