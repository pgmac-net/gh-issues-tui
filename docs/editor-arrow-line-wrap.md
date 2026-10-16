# Arrow keys wrap across body lines (#197)

Ticket: pgmac-net/gh-issues-tui#197 (bug, "Editor widget")
PR: pgmac-net/gh-issues-tui#199

## Behaviour

In the multi-line body/comment editor (`BodyEditor`, `src/tui/app/editor.rs`):

- `←` at column 0 moves to the end of the previous line. On the first line it stays put.
- `→` at the end of a line moves to column 0 of the next line. On the last line it stays put.
- `Ctrl+←` / `Ctrl+→` follow the same rule at line edges. Otherwise they step by word.

Single-line inputs (`InputState`: search, filters, pickers, the new-issue title) are unchanged.

## Implementation

`BodyEditor::left`, `right`, `word_left` and `word_right` check whether the cursor is at a line edge with a neighbouring line. If so they move `line` by one and set the cursor to the new line's start or end. Otherwise they delegate to the per-line `InputState` method, as before. This mirrors the cross-line logic `backspace` and `delete_char` already had.

Visual (soft-wrapped) rows need no special handling. The cursor is a logical char index within a line, so `←`/`→` already flow across wrapped rows of one line.

## Tests

`src/tui/app/tests.rs`, `body_*` tests: mid-line moves, column 0 and end-of-line crossings, first and last line no-ops, empty lines, multibyte text, and the Ctrl variants.

## Decisions

- Ctrl+arrows also wrap. The ticket only named plain arrows; the user chose consistency with them.
- No change to `Up`/`Down` (`up_visual`/`down_visual`), which already move by visual row.
