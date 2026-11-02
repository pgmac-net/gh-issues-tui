## Context

The measurements behind this change, taken on `main` at commit `559f028`:

```
prod lines (test modules excluded)
tui/app.rs      ████████████████████████████████████ 2102  (281 fns, App = 46 fields)
tui/event.rs    ████████████████████████████         1675  (89 fns)
tui/ui.rs       ████████████████████████             1448  (68 fns)
github/client   ████████████████████                 1223
linear/client   █████████████                         771
jira/client     ███████████                           643
markdown.rs     ███████                               408
provider/types  █████                                 321
...rest         ~1000
                                                    ─────
                                                    ~9617  prod   (~4900 test)
```

Duplication located by inspection, not by a similarity tool — each of these was read and confirmed:

| What | Where | Verdict |
|---|---|---|
| `join_error_messages` | `github/client.rs:581`, `linear/client.rs:505` | byte-identical |
| `parse_at<T>` | `github/client.rs:594`, `linear/client.rs:518` | byte-identical |
| `synthetic_priority_labels` | `linear/mod.rs:17`, `jira/mod.rs:44` | identical bar the prefix const |
| synthetic-prefix strip | `linear/mod.rs:31`, `jira/mod.rs:59` | identical bar the prefix const and return type |
| ctor + rate-limit store + `graphql()` | `github/client.rs:59-145`, `linear/client.rs:24-105` | ~90% shared, provider-specific tails |
| rate-limit message format | `github/client.rs:101`, `linear/client.rs:78` | same format string, written twice |
| picker popup draw | `ui.rs` × 5 (`draw_select_popup`, `draw_priority_popup`, `draw_labels_popup`, `draw_pr_picker_popup`, `draw_form_choice_popup`) | same 5-step body, 4 varying inputs |

Each TUI feature is currently smeared across all three big files — state in `app.rs`, keys in `event.rs`, drawing in `ui.rs`:

```
                app.rs          event.rs             ui.rs
 PR summary  →  pr_* fields  ·  handle_pr_*_key   ·  draw_pr_summary_popup
 detail pane →  detail_*     ·  detail_scroll     ·  draw_detail*
 issue form  →  IssueForm    ·  handle_form_*     ·  draw_issue_form
 picker      →  select_*     ·  picker_common_key ·  draw_*_popup
```

## Goals / Non-Goals

**Goals:**
- Remove the confirmed verbatim duplication, so a fourth backend costs a trait impl plus a value map rather than a copied client.
- Make screen geometry and the PR popup's line model single-sourced, so renderer and key handler cannot drift.
- Get every file under roughly 600 production lines, so a feature can be found by filename.
- Fix the PR-summary row-offset bug that the mirroring already caused.

**Non-Goals:**
- No performance work. Issue #87 defers it explicitly.
- No behaviour changes beyond the row-offset correction.
- Not restructuring `App`'s 46 fields into sub-structs — deferred (see Decisions).
- Not merging the picker key-handler pairs — rejected (see Decisions).
- Not touching `linkmap.rs`, `markdown.rs`, `theme.rs`, `config.rs`, or `cwd_repo.rs`.

## Decisions

**Decision: characterisation tests before any refactor, and the goldens are then frozen.**

The suite is large but unevenly distributed: `ui.rs` has 336 test lines against 1448 production lines, and popup rendering is barely covered. Refactoring rendering code with no render assertions means "it compiles and clippy is quiet" becomes the only signal, which is not a behaviour-preservation argument. Phase 0 adds the missing assertions first. During Phases A–D those goldens must not be edited — a golden that needs changing means the refactor changed behaviour, and the correct response is to investigate rather than update the expectation. The one sanctioned exception is the long-body PR case, which is written to document the bug in Phase 0 and flips to asserting the fix in Phase C.

Rejected: "refactor first, add tests where it breaks." That inverts the safety argument — the tests would then be written to match post-refactor behaviour, which proves nothing about preservation.

**Decision: split by layer, not by feature.**

Two axes were weighed:

1. **By layer** (chosen) — `app/{filters,editor,form,detail,pr}.rs`, `event/{mod,spawn,keys/*}.rs`, `ui/{list,detail,popups,form,pr,widgets}.rs`. Preserves today's mental model and everything `CLAUDE.md` already documents; the diff is close to pure movement.
2. **By feature** (vertical slices) — `tui/pr_summary/{state,keys,draw}.rs` and so on. Better cohesion for the smearing shown above, and arguably where this codebase eventually wants to be. Rejected for *this* change: it is a conceptual break from the documented architecture at the same time as four other changes are landing, which makes the diff hard to review and hard to revert in pieces.

Layer-splitting does not preclude the feature axis later; it makes the seams visible, which is the prerequisite for judging whether the feature axis is actually better here.

**Decision: fix the PR-summary bug by removing the mirror, not by correcting the arithmetic.**

The immediate bug could be patched by making `pr_targets` account for wrapping. That leaves two places computing the same thing and re-arms the trap. Instead, one function builds `Vec<PrRow>` where `PrRow { line: Line, url: Option<String> }`; the renderer draws `rows[i].line` and `pr_targets` derives `PrTarget { url, line: i }` from the same vector. The offsets cannot disagree because there is only one of them.

Wrapping is then owned by `linkmap::wrap` with `Paragraph` wrapping switched off — exactly the precedent the detail pane already set and that `CLAUDE.md` already documents as the house rule. `Theme` affects styling but not row count or URLs, so measuring against `Theme::default()` is sound; `ui::body_content_height` already relies on this same property.

**Decision: `layout.rs` holds pure fns over `Rect`, rather than caching geometry on `App`.**

The alternative was to have `ui::draw` record the areas it computed onto `App` for the key handler to read back. Rejected: it breaks the standing invariant that draw code performs no state mutation (`ui.rs` is "pure render from `&App`"), and it introduces a frame-ordering dependency where the key handler's correctness depends on a draw having happened first. Pure functions called by both sides give single-sourcing with neither drawback.

**Decision: leave the picker key-handler pairs duplicated.**

`handle_select_field_key`/`handle_form_select_key` and their `_multi` counterparts look like duplication, but the genuinely shared part is already factored into `picker_common_key`. What remains differs in commit target and return mode. Unifying them would require passing closures or a trait object to describe "where does this pick go and what mode do we return to" — trading eight readable lines for an indirection that reads worse. Duplication is not automatically a defect; this is the case where the abstraction costs more than the repetition.

**Decision: defer the `App` field grouping.**

`App`'s 46 fields cluster into at least six concerns (data/view, picker, detail pane, PR summary, editor, chrome), and `issue_form: Option<IssueForm>` already demonstrates the pattern working. But every `app.pr_scroll` becomes `app.pr.scroll` across all three TUI files and their tests — a very large mechanical diff, landing on top of four other changes. Better as its own change, once Phase D has separated the files so each group's true extent is visible.

## Risks / Trade-offs

- **[Risk]** Phase D is a large diff and could hide an unintended logic change among the movement. → **Mitigation**: `git diff --stat` must show near-symmetric additions and deletions, with changed lines confined to `use`, `mod`, and visibility. Any semantic diff in Phase D is treated as a mistake, not as opportunistic cleanup.
- **[Risk]** Phase A touches all three backends but only GitHub can be verified live — no Linear or Jira instance is available. → **Mitigation**: Phase 0 adds payload goldens for all three, and the existing `linear/mod.rs`, `jira/mod.rs` and client unit tests must pass unchanged. The limitation is stated explicitly in the tasks rather than papered over.
- **[Risk]** Freezing Phase 0's goldens can mask a case where the refactor is correct and the original golden captured incidental behaviour. → **Mitigation**: the rule is "investigate before changing", not "never change" — but a changed golden must be justified in the PR description, not edited silently.
- **[Trade-off]** Splitting by layer keeps each feature spread across three directories. Accepted for now; naming it here so the eventual feature-axis conversation starts from a recorded decision rather than a rediscovery.
- **[Trade-off]** The test suite grows relative to production code. Accepted — the goldens are permanent regression protection for a UI that currently has almost none.
