# Consistent pane keys

Tracks [gh-issues-tui#217](https://github.com/pgmac-net/gh-issues-tui/issues/217).
Decision record: [ADR 0006](adr/0006-pane-keys-are-tree-semantics-and-the-comments-viewport-edge-scrolls.md).

## What changed

The tree (issue list) and the detail pane now share one key model. It is the
model of a file tree: `↑`/`↓` move the selection, `←` goes **out** one level,
`→` goes **in** one level, and `Space`/`Enter` toggle. Every one of these keys
acts on the **focused** pane only. The other pane's selection stays visible
but is dimmed.

The original report in #217 (`[`/`]` not folding) was a stale v0.14.3
binary. v0.14.4 already behaved as designed. The ticket became the wider
consistency fix described here.

## Keys

### Both panes

| Key | Tree focused | Detail pane focused |
|---|---|---|
| `↑`/`↓`, `k`/`j` | previous / next row | row cursor over body and comments (below) |
| `PgUp`/`PgDn` | 15 rows | one viewport of `↑`/`↓` steps |
| `g`/`Home` | first row | select the body, scroll both regions to the top |
| `G`/`End` | last row | select the last comment, scroll the thread to its end |
| `[` / `]` | collapse / expand every repo | fold / unfold every comment |
| `Tab`/`Shift+Tab` | switch into the pane | previous / next comment header (wraps) |

### `←` / `→` in the tree

| Selected | `←` | `→` |
|---|---|---|
| Issue | select its repo header (no collapse) | open or focus the detail pane |
| Expanded header | collapse it | select its first issue |
| Collapsed header | no-op | expand it |

### `←` / `→` in the detail pane

| Selected | `←` | `→` |
|---|---|---|
| Unfolded comment | fold it | no-op |
| Folded comment | focus the tree | unfold it |
| Body | focus the tree | no-op |

### `Space` and `Enter` (identical)

- Repo header: toggle collapse.
- Issue: same as `→`. `Space` no longer collapses the issue's repo. `Enter`
  keeps your place in an already-open pane.
- Comment: toggle fold.
- Body: no-op.

## The detail pane's row cursor

The body and the comments form one sequence, `Body → Comment(0) → … →
Comment(n-1)`. `↑`/`↓` do not wrap at either end. The selected unit is always
a whole item; there is no per-line cursor.

- On an item whose bottom is still below the viewport, `↓` scrolls one row.
  Once the bottom is visible, the next `↓` selects the next item, scrolling
  only enough to show its header.
- `↑` mirrors that: it scrolls until the item's header is visible, then
  selects the previous item, showing that item's last row.
- The body region scrolls the same way before `↓` leaves it for comment 0.

The comments viewport moves only when the selection reaches an edge, and
only by the minimum needed. Selecting a comment, with `Tab` or with `↓`, no
longer snaps it to the top. Folding or unfolding keeps the selected header on
the same screen row. The view moves only when the thread becomes shorter than
the viewport, or when folding a comment you were reading mid-way would leave
it off screen.

The comments scrollbar shows position in the **whole thread**. It used to
show position within the selected comment (#59).

## Theme

`selected_bg_unfocused` colours the selection in the pane without focus,
with a profile override like the other tokens:

```toml
[color_profiles.gruvbox]
selected_bg_unfocused = "#32302f"
```

With the split closed, the list is the only pane and is always drawn with
`selected_bg`.

## Implementation notes

- **Pure moves.** `DetailState::step`, `page`, `jump_top`, `jump_end`,
  `reveal_selected`, `fold_anchor` and `after_fold` (`tui/app/detail.rs`) take
  a `DetailGeometry`: body content height, body viewport, each comment's
  height, and the comments viewport. The key handler (`keys/detail.rs`,
  `detail_geometry`) measures it with `ui::body_content_height` and
  `ui::comment_height`, so `app/` still does no wrapping of its own and can't
  drift from the renderer.
- **Edge scroll.** `layout::scroll_into_view(prev, sel_top, sel_bottom,
  viewport, total)` extends `layout::scroll_top` (#189) to a selection that
  spans several rows. A selection taller than the viewport keeps the view
  while the view lies inside it; otherwise its header goes to the top.
- **Folds.** `with_fold` in `keys/detail.rs` takes the header's screen row
  before the fold (`fold_anchor`) and restores it afterwards (`after_fold`).
- **Tree.** `App::tree_left` / `tree_right` (`tui/app/rows.rs`). `tree_right`
  on an issue returns `TreeRight::EnterDetail`, and the handler calls
  `enter_detail` because it may need to spawn a comment fetch.
- **Routing.** In `keys/normal.rs`, every key above is one match arm split on
  `app.focus`. This is what fixed `g`/`G` reaching the tree from the detail
  pane: an unguarded fallback arm next to a guarded one.
- **Removed.** `DetailState::scroll_body`, `scroll_comment` and
  `snap_comment`, plus `keys::detail::{detail_scroll, detail_page_rows,
  snap_after_select, snap_after_fold}`.

## Tests

- `app/detail.rs`: `step`/`page`/`jump_*`/fold over fixture geometry. Covers
  a folded thread (no scroll inside the viewport, one row at each edge, no
  wrap), a comment taller than the viewport, body ↔ comment 0, fold anchoring,
  clamping, and no comments region.
- `layout.rs`: `scroll_into_view`, including a selection taller than the
  viewport and a shrunken terminal.
- `app/tests.rs`: `tree_left`/`tree_right` for every row kind, and
  `detail_left`/`detail_right`.
- `keys/harness.rs`: every pane key × focus × selection. With the detail pane
  focused no key changes `selected` or `collapsed`; with the tree focused no
  key changes `folded_comments` or `comments_scroll`.
- `ui/detail.rs`: `TestBackend` renders of the whole-thread scrollbar and the
  dimmed unfocused selection.
