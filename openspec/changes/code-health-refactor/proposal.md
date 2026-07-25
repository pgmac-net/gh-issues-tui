## Why

Issue [#87](https://github.com/pgmac-net/gh-issues-tui/issues/87): the app grew organically through feature requests with few health checks. The ask is a review focused on **code re-use and readability**; optimisation and speed are explicitly deferred to a later pass.

The codebase is clean at the micro level — 2 `#[allow]`s in the whole tree, no stray `unwrap` on production paths, dense doc comments, ~4.9k test lines against ~9.6k production lines. The problems are structural, and measurable:

- **54% of production code sits in three files.** `src/tui/app.rs` (2102 prod lines, 281 fns), `src/tui/event.rs` (1675), `src/tui/ui.rs` (1448). `App` alone carries 46 public fields spanning at least six unrelated concerns.
- **Verbatim duplication across providers.** `join_error_messages` and `parse_at` are byte-identical in `github/client.rs` and `linear/client.rs`. The synthetic-priority machinery in `linear/mod.rs` and `jira/mod.rs` is identical bar one prefix constant. The client constructor, rate-limit store, and `graphql()` body are ~90% shared between GitHub and Linear.
- **Five near-identical picker popups** in `ui.rs`, differing only by title, width, multi-select flag, and clear label.
- **Layout arithmetic hand-mirrored between renderer and key handler.** `CLAUDE.md` documents this as "keep both in sync" — an invariant enforced by comment alone. It has already drifted into a live bug (below).

The mirroring has produced a real defect, not just a hazard. `App::pr_targets` computes PR-summary row offsets as `8 + s.body.lines().count()`, but `draw_pr_summary_popup` renders through `Paragraph::wrap(Wrap { trim: false })` at inner width 74. Any PR body line longer than 74 columns wraps to two or more rendered rows, so every check and workflow-run offset after it is short — `Tab` highlights the wrong row and scrolls to the wrong place. The detail pane already solved this exact class of problem by owning its wrapping in `tui::linkmap`; the PR popup never got the same treatment.

## What Changes

Five phases, ordered so that behaviour is pinned before it is moved.

- **Phase 0 — characterisation tests.** Pin current behaviour where coverage is thin, before any refactor. Extends the `TestBackend` render harness already present in `ui.rs` tests and the sample-payload harness already present in the provider tests.
- **Phase A — provider dedup.** New `provider/http.rs` (shared HTTP client builder, rate-limit store and message formatter, `join_error_messages`, `parse_at`, `graphql_post`) and `provider/priority.rs` (prefix-parameterised synthetic-priority helpers). Provider-specific behaviour stays with its provider.
- **Phase B — one picker popup.** Five `draw_*_popup` fns collapse into one `draw_picker` taking a `PickerSpec`.
- **Phase C — layout single source of truth.** A `PrRow { line, url }` model becomes the sole origin of both the rendered popup and its open-able targets, fixing the wrap-drift bug. A new `tui/layout.rs` holds the pure geometry fns that `ui::draw` and `event::detail_metrics` both call, replacing the hand-copied arithmetic.
- **Phase D — split the big three by layer.** `app.rs`, `event.rs`, `ui.rs` become directories of focused modules. Pure `mod` extraction: moves, `use` fixes and visibility only, no logic edits.

- **BREAKING**: none. No user-visible behaviour changes, no config changes, no CLI changes. The single intended behaviour change is the correction of the PR-summary row-offset bug, which today produces a wrong highlight position.

## Capabilities

### New Capabilities

- `provider-internals`: what the backend-neutral provider layer owns versus what each backend implements, so a fourth provider costs a trait impl and a value map rather than a copied client.
- `tui-layout`: how screen geometry and line models are derived, and the rule that renderer and key handler must read the same source rather than mirror each other.
- `module-structure`: the size and organisation constraints on the TUI modules.

### Modified Capabilities

(none — `openspec/specs/` is currently empty)

## Impact

- **Affected code**: all of `src/tui/` and all of `src/provider/`, `src/github/`, `src/linear/`, `src/jira/`. This is a wide change by design; it is staged so each phase is independently reviewable and independently revertable.
- **Affected docs**: `CLAUDE.md` — the `tui/` architecture section gains the new file map, and the two "keep both in sync" warnings are deleted because the hazard they describe stops existing.
- **Behaviour**: unchanged, except the PR-summary highlight lands on the correct row for PRs with long body lines.
- **Test suite**: grows. Phase 0's goldens are permanent regression protection, not scaffolding.
- **Deferred, deliberately**: grouping `App`'s 46 fields into sub-structs (`PickerState`, `DetailState`, `PrState`). High churn across all three TUI files plus their tests, and much easier to do well once Phase D has made the seams visible. Tracked as follow-up work, not dropped.
- **Verification limit**: Phase A touches all three backends, but no Linear or Jira instance is available to this project. Their coverage is the Phase 0 payload goldens plus the existing unit tests; live verification covers GitHub only, and the change should say so rather than imply otherwise.
