# Inline new-issue form

Ticket: [pgmac-net/gh-issues-tui#57](https://github.com/pgmac-net/gh-issues-tui/issues/57)

## What it does

`n` opens the new-issue form as a single inline box instead of a row list where
each field opened its own modal. Title and description edit in place; `Tab`/
`Shift+Tab` move between fields and the `[ Create ]`/`[ Cancel ]` buttons at the
bottom, wrapping at both ends.

## Behaviour

- **Title** — inline single-line editor (horizontally scrolls to keep the
  cursor visible). `Enter` moves focus to the next field instead of submitting.
- **Description** — inline multi-line box, four visual rows tall, scrolling to
  keep the cursor's row visible. `Enter` inserts a newline; `Tab`/`Shift+Tab`
  leave the field.
- **Choice fields** (assignees, labels, type, priority, project, milestone) —
  unchanged: `Enter` opens the existing filterable picker popup (single- or
  multi-select). This is the one deliberate exception to "no modals" — these
  option lists are long and benefit from the pickers' type-ahead filter.
- **`[ Create ]`** — `Enter`/`Space` submits when a title is present and the
  repo's picker options have loaded.
- **`[ Cancel ]`** — `Enter`/`Space` discards the form.
- **`Esc`** cancels from any field, matching the rest of the app.
- `j`/`k` no longer move between fields (they're literal characters when the
  title field is focused); movement is `Tab`/`Shift+Tab` only.

## Implementation

| Area | File | Change |
|------|------|--------|
| Form state | `src/tui/app.rs` | `IssueForm.title` changed from `String` to `InputState`; added `ISSUE_FORM_CANCEL_ROW`, `ISSUE_FORM_WIDTH`/`issue_form_width`, `ISSUE_FORM_LABEL_WIDTH`, `ISSUE_FORM_DESC_HEIGHT` |
| Mode/enum cleanup | `src/tui/app.rs` | removed `Mode::IssueFormBody` and `InputKind::FormTitle` — the form has no more sub-modes for text fields |
| Key handling | `src/tui/event.rs` | `handle_issue_form_key` rewritten: `Tab`/`BackTab` always move focus (`next_form_field`/`prev_form_field`, wrapping); the rest dispatches on the focused row (inline edit for title/description, picker-open for choice fields, activate for Create/Cancel) |
| Shared editing helper | `src/tui/event.rs` | new `apply_input_editor_key` (readline-style single-line editing) extracted out of `handle_input_key` and reused for the inline title field |
| Rendering | `src/tui/ui.rs` | `draw_issue_form` rewritten as one `Paragraph` (label + inline value per row, cursor drawn on the focused text field, background-highlight on focused choice fields, a centred `[ Create ]  [ Cancel ]` button line); removed `draw_form_body_popup` |

`IssueForm::is_multi_field`/`is_select_field` and the picker sub-modes
(`Mode::IssueFormSelect`/`IssueFormMulti`) are unchanged — only the container
form and its text fields moved inline.

Covered by unit tests in `src/tui/event.rs`: `Tab`/`Shift+Tab` wrap across all
fields and both buttons, inline title editing plus `Enter`-advances-focus,
inline description editing plus `Enter`-inserts-newline, `[ Create ]`
submitting a valid form, `[ Cancel ]` (`Enter` and `Space`) discarding, and
`Esc` cancelling regardless of the focused field.

## Design decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Choice fields | keep popup pickers | Long, filterable option lists (assignees, labels, etc.) — the ticket's "no modals" rule allows an exception when "absolutely necessary"; a picker with type-ahead is materially better than an inline dropdown here |
| Navigation | `Tab`/`Shift+Tab` only, no `j`/`k` | Text fields now hold real keyboard focus, so `j`/`k` need to be literal characters; a single navigation scheme is simpler than a mode-per-field-type split |
| Description | inline multi-line box, not a popup | Matches the ticket's "stay on the one form" requirement; reuses the existing `BodyEditor` machinery, just rendered inline instead of centred over the form |
