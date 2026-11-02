## Why

Issue [#89](https://github.com/pgmac-net/gh-issues-tui/issues/89), deferred from #87/#88. That change split the TUI into per-concern modules but deliberately left `App`'s field list alone, because doing both at once would have made the diff unreviewable. The split made the seams visible; this acts on them.

`App` carries 46 public fields spanning six unrelated concerns. Four of those concerns are reset **as a set, by hand, in several places** — and the sets deliberately differ:

```rust
clear_pr_state()      // clears all 5 PR fields
open_pr_summary(pr)   // sets target, clears 3, sets mode
close_pr_summary()    // clears 4 — deliberately keeps pr_links
refresh_pr_summary()  // clears 3
```

Nothing records which of those four a newly added PR field belongs in. The same shape exists for the detail pane (`open_detail`, `close_detail`, `reset_detail_scroll`, `switch_org`), the picker (`start_picker`, `picker_filter_clear`) and the inline editor (`cancel_comment`, `submit_comment`). Forgetting one is a live bug class, not a hypothetical — the PR-summary row-drift bug fixed in #88 came from exactly this species of duplicated-by-hand state.

`issue_form: Option<IssueForm>` already demonstrates the pattern working.

## What Changes

Group the four concerns whose fields reset together. Each gets a `Default` and named methods, so a reset's intent is recorded once instead of restated at every call site.

```rust
App {
    // data/view — unchanged, flat
    org, repos, rows, selected, filters, sort_key, …
    // chrome — unchanged, flat
    loading, status, rate_limit, rate_limit_error, …
    // grouped
    detail: DetailState,   // open, sel, comments, body_scroll, comments_scroll
    pr:     PrState,       // links, target, summary, scroll, sel
    picker: PickerState,   // options, idx, filter, multi_selected,
                           // priority_issue, label_issue
    editor: EditorState,   // body, focus, target
}
```

46 fields → 31.

Logic that touches only one group moves onto that group. `filtered_select`, `picker_selected_original` and `clamp_picker_idx` become `impl PickerState`; `reset_detail_scroll`, `select_detail`, `clamp_detail_sel`, `scroll_body`, `scroll_comment`, `snap_comment` become `impl DetailState`; `select_pr_target` and `pr_selected_url` become `impl PrState`. They stop needing a whole `App` to exercise, so they become unit-testable directly.

Methods that genuinely cross groups stay on `App` and delegate — `open_detail` still touches `focus` and clears PR state, `open_pr_summary` still sets `mode`.

- **BREAKING**: none. No user-visible behaviour changes, no config or CLI changes.

## Capabilities

### New Capabilities

- `app-state`: how application state is grouped, and the rule that a group's reset semantics live with the group rather than being restated at each call site.

### Modified Capabilities

(none)

## Impact

- **Affected code**: `src/tui/app/` throughout, plus every reader in `src/tui/event/` and `src/tui/ui/`. Roughly 300 edit sites, nearly all mechanical field-path rewrites.
- **Behaviour**: unchanged. The 345 existing tests must pass, and the characterisation goldens added in #88 must not be edited.
- **Not grouped, deliberately**: the chrome fields (`loading`, `auto_refreshing`, `status`, `rate_limit`, `rate_limit_error`). They account for ~119 reference sites, 72 of them `status` alone — a plain scalar that nothing resets as a set. Wrapping it would be the single largest chunk of churn in the change and would buy neither safety nor readability. Grouping for uniformity's sake is not a goal.
- **Risk to watch**: the differing reset sets above must be **preserved, not flattened**. `close_pr_summary` keeping `pr_links` is deliberate; a `PrState::default()` applied there would be a behaviour change disguised as a cleanup.
