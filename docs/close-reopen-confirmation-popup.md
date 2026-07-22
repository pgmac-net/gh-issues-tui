# Close/reopen confirmation popup (#56, 2026-07-23)

Work driven by pgmac-net/gh-issues-tui#56 [https://github.com/pgmac-net/gh-issues-tui/issues/56](https://github.com/pgmac-net/gh-issues-tui/issues/56), delivered in PR #64 [https://github.com/pgmac-net/gh-issues-tui/pull/64](https://github.com/pgmac-net/gh-issues-tui/pull/64) on branch 56-close-confirm-modal.

## Problem

Pressing `x` to close or reopen an issue prompted for a y/n answer on the status line — a single dim line at the bottom of the screen, easy to miss, especially since it looks similar to the ordinary status messages shown there the rest of the time.

## Fix

**`src/tui/app.rs`**
- Added `ConfirmChoice { Yes, No }`, mirroring the existing `CommentFocus` pattern.
- Added `confirm_choice: ConfirmChoice` on `App`, reset to `No` (the safe default) each time `Mode::ConfirmState` is entered.

**`src/tui/ui.rs`**
- New `draw_confirm_popup`: a centered, bordered popup (`centered()` + `Clear`, sized to the message) titled `close issue` / `reopen issue` depending on the selected issue's current state, with the message `close issue #N?` / `reopen issue #N?` and a `[ Yes ]  [ No ]` button row.
- The focused button reuses the reversed-video style already used for the `[ Save ]  [ Cancel ]` row in the inline comment editor, for a consistent look across the app's two button-row popups.
- Removed the old y/n line from `draw_bottom_line`, which now unconditionally shows the status message (previously conditional on not being in `ConfirmState`).

**`src/tui/event.rs`**
- Reworked `handle_confirm_key`:
  - `←`/`→`/`Tab`/`h`/`l` toggle focus between Yes and No without leaving the popup.
  - `Enter` activates whichever button has focus.
  - `y` = immediate close/reopen; `n`/`Esc` = immediate cancel — both bypass focus state, preserving the original keyboard shortcuts for anyone who already has the y/n muscle memory from before this change.
  - The actual mutation logic (unchanged) was extracted into `confirm_toggle_state`, shared by the `y` shortcut and Enter-on-Yes.

## Tests

8 new unit tests:
- `src/tui/event.rs`: focus toggling via all five keys, Enter-on-No cancels without mutating, Enter-on-Yes and the `y` shortcut both reach the mutation path (`#[tokio::test]`, since `with_issue` spawns a task), `n`/`Esc` cancel regardless of which button has focus.
- `src/tui/ui.rs`: render tests using `ratatui::backend::TestBackend` — popup title/message text for both open (→ close) and closed (→ reopen) issues, and a reversed-video modifier check confirming the highlighted button actually tracks `confirm_choice`.

Full suite: 198 passed (190 pre-existing + 8 new). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` both clean.

## Docs updated

- `README.md` — `x` keybinding row updated to describe the popup and its keys.
- `docs/architecture.md` — `ConfirmState` mode description updated.

## Deviations from plan

None — implemented as planned and approved on the ticket.
