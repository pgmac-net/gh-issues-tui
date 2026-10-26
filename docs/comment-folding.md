# Comment folding

Tracks [gh-issues-tui#210](https://github.com/pgmac-net/gh-issues-tui/issues/210).

Long comment threads are hard to skim, so each comment in the detail pane can
be **folded** down to a single header row, and unfolded again.

## Terminology

*Fold* applies to comments in the detail pane. *Collapse* applies to repo
groups in the issue list. The two share keys (below) but are separate state
and are deliberately named differently.

## Behaviour

A folded comment is its header rule alone, marked `▸`, with the number of rows
unfolding would reveal:

```
── ▸ pgmac · 2026-10-22 10:58 (12 lines) ──────────
── ▾ bob · 2026-10-23 09:01 ───────────────────────
Body text of the open comment…
────────────────────────────────────────────────────
```

An open comment is marked `▾` and is otherwise unchanged. The count is the
body's wrapped row count at the current pane width, so it matches what
unfolding shows. Folding also drops the footer rule and trailing blank line.

Keys, **detail pane focused** (with the list focused they keep their
repo-group meaning):

| Key | Action |
|---|---|
| `Space` | fold/unfold the selected comment (no-op while the body is selected) |
| `[` | fold every comment in the thread |
| `]` | unfold every comment in the thread |

Details:

- Everything starts expanded; newly added comments start expanded.
- `Tab`/`Shift+Tab` still stop on folded comments, which is how you reach one
  to unfold it. `j`/`k`/`PgUp`/`PgDn` on a folded comment do nothing: one row
  always fits.
- After any fold or unfold, the selected comment's header is snapped to the
  top of the comments region. With the body selected, the scroll offset is only
  clamped to the thread's new, shorter height.
- `e` still edits a folded comment; it stays folded after the thread refetch.

## Implementation notes

- State is `App.folded_comments: HashSet<String>` of comment ids. It is keyed by
  id, not position, so it survives the refetch after an add or edit and
  navigating away and back. It is session-only (never written to disk) and is
  cleared by `switch_org`. It lives on `App`, not `DetailState`, because
  `open_detail` resets `DetailState` wholesale.
- `ui::comment_height` and `ui::comment_offset` take the fold state, and the
  renderer and the key handlers' scroll clamps both go through them, so a
  folded card's height can't drift between drawing and clamping
  (see `docs/detail-pane-two-region-scroll.md`).
- Fold-all acts on the loaded thread's ids only; other issues' folds are kept.
