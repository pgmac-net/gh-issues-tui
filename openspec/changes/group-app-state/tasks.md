## 1. Baseline

- [x] 1.1 Record the function inventory and test roster from `main` — the same before/after evidence used in #88
- [x] 1.2 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green before starting

## 2. PickerState

- [x] 2.1 Add `PickerState { options, idx, filter, multi_selected, priority_issue, label_issue }` with `Default` in `app/picker.rs`
- [x] 2.2 Move `filtered_select`, `picker_selected_original`, `clamp_picker_idx`, `start_picker`, `picker_filter_push`/`_backspace`/`_clear` onto `impl PickerState`
- [x] 2.3 Rewrite the ~95 reference sites across `app/`, `event/` and `ui/`
- [x] 2.4 Add unit tests for the filter/index arithmetic now that it needs no `App`

## 3. DetailState

- [x] 3.1 Add `DetailState { open, sel, comments, body_scroll, comments_scroll }` with `Default` in `app/detail.rs`
- [x] 3.2 Move `reset_detail_scroll`, `select_detail`, `clamp_detail_sel`, `scroll_body`, `scroll_comment`, `snap_comment`, `detail_comment_count` onto `impl DetailState`
- [x] 3.3 Keep `open_detail`, `close_detail`, `enter_detail`, `start_comment_editor`, `start_edit_selected_card` on `App` — they touch `focus`, `mode` or PR state — and have them delegate
- [x] 3.4 Rewrite the ~96 reference sites

## 4. PrState

- [x] 4.1 Add `PrState { links, target, summary, scroll, sel }` with `Default` in `app/pr.rs`
- [x] 4.2 Add `PrState::open(pr)`, `close()`, `refresh()` reproducing the current subsets **exactly** — `close()` retains `links`
- [x] 4.3 Replace `clear_pr_state` with `self.pr = PrState::default()`
- [x] 4.4 Move `select_pr_target` and `pr_selected_url` onto `impl PrState`
- [x] 4.5 Rewrite the ~60 reference sites
- [x] 4.6 Confirm the existing test asserting `pr_links` survives a summary close still passes unedited

## 5. EditorState

- [x] 5.1 Add `EditorState { body, focus, target }` with `Default` in `app/editor.rs`
- [x] 5.2 Rewrite the ~50 reference sites; `cancel_comment` and the reset half of `submit_comment` become `EditorState::default()`

## 6. Verification

- [x] 6.1 Function inventory byte-identical to the 1.1 baseline, except for the methods deliberately moved onto the new groups
- [x] 6.2 Test roster byte-identical apart from the new `PickerState` tests; all previously existing tests pass **unedited**
- [x] 6.3 No characterisation golden from #88 edited
- [x] 6.4 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green
- [x] 6.5 Confirm `App`'s field count is 31 and the chrome fields are still flat
- [x] 6.6 Drove the TUI against `pgmac-net`: detail pane and both scroll regions, comment cards, priority/labels pickers, new-issue form, and the PR summary opened, Tab'd, closed and reopened. The **multi-link picker path was not driven** — no loaded issue carries two PR links; it is covered by `close_keeps_the_discovered_links` in `app/pr.rs`
- [x] 6.7 Update `CLAUDE.md`'s `app/` description with the grouped shape and the reason the chrome fields are not grouped
