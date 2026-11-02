## 0. Characterisation tests (must all pass before any refactor)

- [x] 0.1 Popup render goldens in `src/tui/ui.rs` tests, extending the existing `TestBackend` harness (`render_confirm_buffer`, `detail_render_string`): one render assertion each for `SelectField`, `SelectFieldMulti`, `PrioritySet`, `LabelsSet`, `PrPicker`, `IssueFormSelect`, `IssueFormMulti` — asserting title text, popup width, and the `[x]` / cursor markers
- [x] 0.2 Layout arithmetic assertions: `detail_split`, the 40/60 pane split, and the border insets produce the same regions the renderer uses, at 80×24, 100×32 and 200×60
- [x] 0.3 PR summary row model: assert `App::pr_targets()` offsets against a rendered buffer for a summary carrying checks, PR runs and default-branch runs — short-body case as the passing baseline
- [x] 0.4 PR summary long-body case (a body line over 74 columns) written to **document the current wrong offsets**, with a comment naming it as the bug repro that Phase C flips
- [x] 0.5 Provider mapping goldens: one `org_issues`-shaped payload per backend (github/linear/jira) asserted down to `Vec<RepoIssues>`
- [x] 0.6 Provider error-path assertions per backend: rate limit, API error joining, shape error
- [x] 0.7 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all green

## 1. Phase A — provider dedup

- [x] 1.1 Add `src/provider/http.rs`: `build_http_client(auth_header_value)`, `RateLimitStore` (get/set + the single rate-limit message formatter), `join_error_messages`, `parse_at`, `graphql_post`
- [x] 1.2 Move `join_error_messages` and `parse_at` out of `github/client.rs`; delete the byte-identical copies from `linear/client.rs`
- [x] 1.3 Convert `github::Client` to the shared builder, store and `graphql_post`, keeping `graphql_with_backoff`, the resource-limit detection and the HTTP-200 rate-limit detection in place
- [x] 1.4 Convert `linear::Client` likewise, keeping `errors_contain_ratelimit` in place
- [x] 1.5 Convert `jira::Client` to the shared builder and store only (REST — no `graphql_post`)
- [x] 1.6 Add `src/provider/priority.rs`: `synthetic_priority_labels(prefix)` and `strip_synthetic_prefix(prefix, id)`
- [x] 1.7 Rewire `linear/mod.rs` and `jira/mod.rs` onto the shared helpers, each keeping its own prefix const and its own value mapping table
- [x] 1.8 Confirm the pre-existing tests in `linear/mod.rs`, `jira/mod.rs` and all three client modules pass **unedited**
- [x] 1.9 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all green

## 2. Phase B — one picker popup

- [x] 2.1 Add `PickerSpec { title, width, multi, clear_label }` and `draw_picker` to `src/tui/ui.rs`
- [x] 2.2 Delete `draw_select_popup`, `draw_priority_popup`, `draw_labels_popup`, `draw_pr_picker_popup`, `draw_form_choice_popup`; point the `match app.mode` arms in `ui::draw` at `draw_picker`
- [x] 2.3 Confirm task 0.1's goldens pass **unedited** — including the PR picker's width of 60 and the form pickers' clear label of "none"
- [x] 2.4 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all green

## 3. Phase C — layout single source of truth

- [x] 3.1 Add `PrRow { line, url }` and `pr_summary_rows(&PrSummary, &Theme) -> Vec<PrRow>` to `ui.rs`
- [x] 3.2 Rewrite `draw_pr_summary_popup` to render the row model, pre-wrapping via `linkmap::wrap` with `Paragraph` wrapping switched off
- [x] 3.3 Rewrite `App::pr_targets` to derive targets from the row model by position; delete the hand-mirrored offset arithmetic and its comment
- [x] 3.4 Flip task 0.4's long-body case from documenting the bug to asserting the fix — confirm it fails before 3.1–3.3 and passes after, and record both observations
- [x] 3.5 Add `src/tui/layout.rs` with pure `frame(area)`, `panes(main, detail_open)` and `detail_regions(detail)`; move `detail_split` there from `app.rs`
- [x] 3.6 Point `ui::draw` at the layout functions in place of its inline `Layout` calls
- [x] 3.7 Reduce `event::detail_metrics` to a wrapper over the layout functions; delete its copied border and status-line arithmetic
- [x] 3.8 Delete both "keep both in sync" warnings from `CLAUDE.md`
- [x] 3.9 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all green

## 4. Phase D — split the big three by layer

- [x] 4.1 Split `app.rs` into `app/{mod,filters,editor,form,detail,pr}.rs`, tests moving with their code
- [x] 4.2 Split `event.rs` into `event/{mod,spawn}.rs` and `event/keys/{normal,filter,form,detail,pr,shared}.rs`
- [x] 4.3 Split `ui.rs` into `ui/{mod,list,detail,popups,form,pr,widgets}.rs`
- [x] 4.4 Re-export from each `mod.rs` so existing paths (`ui::body_content_height`, `ui::comment_offset`, and the rest) keep resolving unchanged
- [x] 4.5 Confirm no file exceeds ~600 production lines
- [x] 4.6 Review `git diff --stat`: additions and deletions near-symmetric, changed lines confined to `use` / `mod` / visibility
- [x] 4.7 Update the `tui/` architecture section of `CLAUDE.md` with the new file map
- [x] 4.8 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all green

## 5. Verification

- [x] 5.1 Drive the TUI with the `verify` skill after Phase C: detail pane open, body and comment scrolling, Tab between comment cards, PR summary opened and Tab'd through its rows, each picker opened (`p`, `l`, filter menu, form fields), new-issue form navigation
- [x] 5.2 Repeat 5.1 after Phase D — detail pane, comment cards, priority/labels pickers, new-issue form and PR summary Tab navigation all behave identically to the Phase C drive
- [x] 5.3 Run against a real GitHub org (`pgmac-net`, 121 issues / 57 repos): fetch, filter clear + repo picker, detail pane, comment thread and PR summary all confirmed unchanged. **Mutations were deliberately not driven** — they would alter real issues; they stay covered by unit tests only
- [x] 5.4 Recorded in the Phase A commit message and the proposal: Linear and Jira have **no live verification** (no instance available); their coverage is the payload goldens plus existing unit tests, all of which pass unedited
- [x] 5.5 Confirm no Phase 0 golden was edited except task 0.4's, and justify that one in the PR description
