# Move an issue to another repo (#135)

Work driven by pgmac-net/gh-issues-tui#135 [https://github.com/pgmac-net/gh-issues-tui/issues/135](https://github.com/pgmac-net/gh-issues-tui/issues/135), delivered on branch `135-move-issue-to-repo`.

## Problem

There was no way to move an issue filed in the wrong repo to the right one without leaving the TUI — closing it and re-filing elsewhere loses the comment thread, the author, and the original timestamps.

## Decisions (grilled before planning)

- **Same-owner transfers only.** GitHub's `transferIssue` hard-rejects cross-owner moves ("you can only transfer issues between repositories owned by the same user or organization account", confirmed against GitHub's docs and by introspecting `TransferIssueInput` on the live API). Emulating a cross-owner move (create + replay comments + close) would rewrite author attribution and timestamps into a lossy re-post wearing a move's clothes, so it was ruled out rather than approximated. A useful side effect: every legal target is already in `App::repos` — the org fetch never needed a second request for the picker.
- **GitHub and Linear ship; Jira doesn't.** Linear's `IssueUpdateInput.teamId` field was confirmed by introspecting the live Linear API. Jira Cloud has no per-issue move at all — only a Beta `POST /rest/api/3/bulk/issues/move`, which is asynchronous (returns a task id to poll) and requires an explicit issue-type and status mapping per target project. That's its own ticket; Jira inherits the capability's `Unsupported` default.
- **The provider resolves the target name to an id, not the TUI.** `move_issue(issue_id, org, target)` takes the human-readable repo/team name; each provider does its own id lookup internally. The TUI layer never touches a node id, so there's no new `AppEvent`, no second async round-trip, and no staleness guard beyond the one already needed for the confirm popup.
- **Picker, then a confirm popup that names the consequences.** A transfer renumbers the issue and notifies everyone mentioned in it, and is undoable only by moving it back — heavier than `x` (close/reopen), which already gets a confirm popup. The popup states the destination and both side effects rather than adding a bare keystroke.
- **`createLabelsIfMissing: true`.** Without it, any label absent from the target repo — including `priority:*`/`status:*`, which this app's sort/colour/filter model is built on — is dropped silently. Trade-off accepted: labels recreated this way get GitHub's default colour, not the source repo's.
- **The cursor doesn't follow the moved issue.** GitHub doesn't document whether a transferred issue keeps its GraphQL node id. Not following it behaves identically either way and reuses `set_data`'s existing vanished-issue fallback; the status line names the destination instead (`moved to <repo>`).

## Fix

**`src/provider/mod.rs`**
- New capability pair on `IssueProvider`, mirroring `supports_pr_summary`/`pull_request`: `supports_move() -> bool` (default `false`) and `move_issue(issue_id, org, target) -> Result<()>` (default `Err(ProviderError::Unsupported(...))`).

**`src/github/client.rs`**
- `Client::move_issue`: looks up `repository(owner:,name:){id}` for the target, then `transferIssue(input: {issueId, repositoryId, createLabelsIfMissing: true})`.
- `IssueProvider` impl: `supports_move() -> true`, delegates to the inherent method.

**`src/linear/client.rs`**
- `Client::move_issue`: looks up the target team's id via `teams(filter: {key: {eq:}})` (the same shape `real_repo_labels` already uses), then `issueUpdate(id, {teamId})`.
- `IssueProvider` impl: `supports_move() -> true`, delegates to the inherent method.

**`src/jira/client.rs`** — untouched; inherits the trait's `Unsupported` default.

**`src/tui/app/mode.rs`**
- `Mode::MovePicker` / `Mode::ConfirmMove`.
- `PendingMove { issue_id, target }`: a move committed from the picker, awaiting confirmation. Doesn't live inside `Mode` — a `String` payload would cost `Mode` its `Copy` bound, unlike `HarnessConfirm::Kill(SessionId)` — so it's `App::pending_move: Option<PendingMove>` instead, captured at picker-commit time so a refetch or selection change while the confirm popup is open can't retarget the mutation onto a different issue.

**`src/tui/app/picker.rs`**
- `App::move_targets()`: every loaded repo except the selected issue's own.
- `App::open_move_picker(targets)`: starts the picker and enters `Mode::MovePicker`.

**`src/tui/event/spawn.rs`**
- `with_issue`'s `done_msg` widened from `&'static str` to `impl Into<String> + Send + 'static`, so the move mutation can report its destination (`"moved to {target}"`) without changing any other call site (a `&'static str` literal still satisfies the bound).

**`src/tui/event/keys/move_issue.rs`** (new)
- `handle_move_picker_key`: Enter on a target captures `PendingMove` and enters `Mode::ConfirmMove`; Esc cancels.
- `handle_confirm_move_key`: mirrors `handle_confirm_key` (arrow/Tab/h/l toggle focus, `y`/Enter-on-Yes commits, `n`/Esc/Enter-on-No cancels). Commit re-checks the captured issue id against the current selection before dispatching — the same "still the target?" guard `handle_priority_set_key`/`handle_labels_set_key` already use — and reports `"selection changed — issue not moved"` if it drifted.

**`src/tui/event/keys/normal.rs`**
- `m`: no-op on a repo header (no selected issue); a status message if `!client.supports_move()`; a status message if `move_targets()` is empty (single-repo org); otherwise opens the picker.

**`src/tui/event/keys/mod.rs`** — dispatches the two new modes.

**`src/tui/ui/popups.rs`**
- `PickerSpec::move_target()` for the picker popup.
- `draw_confirm_move_popup`: states `move #N to <repo>?` plus `Gets a new number; mentioned users are notified.`
- Help table: `m — move issue to another repo`.

**`src/tui/ui/mod.rs`** — dispatches `Mode::MovePicker`/`Mode::ConfirmMove` to the above.

## Tests

7 new unit tests in `src/tui/event/keys/move_issue.rs`: `move_targets()` excludes the issue's own repo and is empty in a single-repo org; Enter on the picker captures the right `PendingMove` and opens the confirm popup; Esc on the picker cancels without setting one; confirm Enter-on-No cancels without mutating; confirm Enter-on-Yes reaches the mutation path (`#[tokio::test]`, `with_issue` spawns a task); a `PendingMove` whose issue id no longer matches the selection is dropped with a status message instead of mutating.

Plus the capability-defaults test in `src/provider/mod.rs` extended to cover `supports_move`/`move_issue`.

Not added: HTTP-level tests asserting the exact GraphQL mutation payload sent to GitHub/Linear. No other mutation in either client (`set_state`, `set_labels`, `set_assignees`, …) has one either — the repo has no mock-HTTP dev-dependency, and `github/client.rs`'s existing tests cover response DTO parsing only (see `docs/architecture.md`'s Testing section). Adding one for this method alone would be new test infrastructure the rest of the mutation surface doesn't have, not parity with it.

Full suite: 507 passed (500 pre-existing + 7 new). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` both clean.

## Docs updated

- `README.md` — intro sentence, `m` keybinding row, a "Moving an issue to another repo" section, and the Linear/Jira capability tables.
- `docs/architecture.md` — capability section, Linear/Jira provider sections, Mutations table, a new "Moving an issue to another repo" section, the Modes list, and the Testing paragraph.
- This file.

## Deviations from plan

- The plan's test list included "GitHub/Linear mutation payload shape" tests. Skipped — see the Tests section above; no other capability method in this repo has that kind of test, and adding one only for `move_issue` isn't parity, it's new infrastructure for one method.
