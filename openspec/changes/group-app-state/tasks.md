## 1. Baseline

- [ ] 1.1 Record the function inventory and test roster from `main` — the same before/after evidence used in #88
- [ ] 1.2 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green before starting

## 2. PickerState

- [ ] 2.1 Add `PickerState { options, idx, filter, multi_selected, priority_issue, label_issue }` with `Default` in `app/picker.rs`
- [ ] 2.2 Move `filtered_select`, `picker_selected_original`, `clamp_picker_idx`, `start_picker`, `picker_filter_push`/`_backspace`/`_clear` onto `impl PickerState`
- [ ] 2.3 Rewrite the ~95 reference sites across `app/`, `event/` and `ui/`
- [ ] 2.4 Add unit tests for the filter/index arithmetic now that it needs no `App`

## 3. DetailState

- [ ] 3.1 Add `DetailState { open, sel, comments, body_scroll, comments_scroll }` with `Default` in `app/detail.rs`
- [ ] 3.2 Move `reset_detail_scroll`, `select_detail`, `clamp_detail_sel`, `scroll_body`, `scroll_comment`, `snap_comment`, `detail_comment_count` onto `impl DetailState`
- [ ] 3.3 Keep `open_detail`, `close_detail`, `enter_detail`, `start_comment_editor`, `start_edit_selected_card` on `App` — they touch `focus`, `mode` or PR state — and have them delegate
- [ ] 3.4 Rewrite the ~96 reference sites

## 4. PrState

- [ ] 4.1 Add `PrState { links, target, summary, scroll, sel }` with `Default` in `app/pr.rs`
- [ ] 4.2 Add `PrState::open(pr)`, `close()`, `refresh()` reproducing the current subsets **exactly** — `close()` retains `links`
- [ ] 4.3 Replace `clear_pr_state` with `self.pr = PrState::default()`
- [ ] 4.4 Move `select_pr_target` and `pr_selected_url` onto `impl PrState`
- [ ] 4.5 Rewrite the ~60 reference sites
- [ ] 4.6 Confirm the existing test asserting `pr_links` survives a summary close still passes unedited

## 5. EditorState

- [ ] 5.1 Add `EditorState { body, focus, target }` with `Default` in `app/editor.rs`
- [ ] 5.2 Rewrite the ~50 reference sites; `cancel_comment` and the reset half of `submit_comment` become `EditorState::default()`

## 6. Verification

- [ ] 6.1 Function inventory byte-identical to the 1.1 baseline, except for the methods deliberately moved onto the new groups
- [ ] 6.2 Test roster byte-identical apart from the new `PickerState` tests; all previously existing tests pass **unedited**
- [ ] 6.3 No characterisation golden from #88 edited
- [ ] 6.4 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green
- [ ] 6.5 Confirm `App`'s field count is 31 and the chrome fields are still flat
- [ ] 6.6 Drive the TUI with the `verify` skill: detail pane and its scroll regions, comment cards, all pickers, the new-issue form, and the PR summary — including closing and reopening the PR picker to exercise the retained `links`
- [ ] 6.7 Update `CLAUDE.md`'s `app/` description with the grouped shape and the reason the chrome fields are not grouped
