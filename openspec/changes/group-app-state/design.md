## Context

Reference counts, measured on `main` at 07bb1d5, to set expectations on diff size and to justify what is left alone:

| Group | Fields | Refs | Reset together in |
|---|---|---|---|
| detail | 5 | 96 | `open_detail`, `close_detail`, `reset_detail_scroll`, `switch_org` |
| picker | 4 + 2 guards | 95 | `start_picker`, `picker_filter_clear` |
| pr | 5 | 60 | `clear_pr_state`, `open`/`close`/`refresh_pr_summary` |
| editor | 3 | 50 | `cancel_comment`, `submit_comment` |
| **chrome** | 5 | **119** | *nothing* |

The chrome row is why this change is not "group everything": it is the largest reference count and the only group with no reset-as-a-set behaviour to capture.

The four reset functions on PR state are the clearest statement of the problem:

```
                    links   target  summary  scroll  sel
clear_pr_state       ✗        ✗        ✗       ✗      ✗
open_pr_summary      ·        set      ✗       ✗      ✗
close_pr_summary     ·        ✗        ✗       ✗      ✗
refresh_pr_summary   ·        ·        ✗       ✗      ✗
```

Three different subsets, one of them keeping `links` on purpose. A sixth field would have to be manually placed into the right three of four bodies, with nothing but reading to say which.

## Goals / Non-Goals

**Goals:**
- Make each group's reset semantics a named method on the group, defined once.
- Preserve the differing subsets above exactly.
- Move group-only logic onto the group so it is testable without an `App`.
- Cut `App` from 46 fields to 31.

**Non-Goals:**
- No behaviour change of any kind.
- Not grouping the chrome fields (rationale above).
- Not touching `Filters`, `IssueForm` or the data/view fields — `Filters` and `IssueForm` are already grouped, and the data/view fields are the actual subject of `App`.
- Not renaming anything user-visible; config keys and CLI flags are untouched.

## Decisions

**Decision: group by "resets together", not by topic.**

The tempting cut is topical — everything PR-ish in one struct, everything list-ish in another. The cut actually taken is behavioural: fields that are cleared or initialised as a unit. That is the property which makes a `Default` meaningful and which makes a forgotten field a bug. It is also why `focus` stays flat despite reading as detail-pane state: `open_detail` sets it, but so do `switch_org` and `cycle_focus`, and it means something with the pane closed.

**Decision: preserve the reset subsets as named methods, not as `Default` everywhere.**

`clear_pr_state` becomes `self.pr = PrState::default()`. The other three do **not**, because they clear less:

```rust
impl PrState {
    fn open(&mut self, pr: PrRef)  // sets target, clears summary/scroll/sel
    fn close(&mut self)            // clears target/summary/scroll/sel, keeps links
    fn refresh(&mut self)          // clears summary/scroll/sel
}
```

Writing `PrState::default()` into `close` would silently drop `pr_links` and change behaviour — a refactor that quietly fixes what it thinks is a bug is worse than the duplication it replaces. If keeping `links` is wrong, that is a separate change with its own justification.

**Decision: field names lose their now-redundant prefixes.**

`self.pr_scroll` becomes `self.pr.scroll`, not `self.pr.pr_scroll`. This is the point of the grouping and it is what makes the resulting code shorter rather than longer. It does mean the mechanical rewrite is a rename as well as a re-path, which is why it is done by exact-match on distinctive names (`\.pr_scroll\b`) rather than by hand.

**Decision: `EditorState` lives in `app/editor.rs` beside `InputState`/`BodyEditor`.**

`editor.rs` currently holds the reusable text-editing widgets. `EditorState` is session state rather than a widget, so this is a slight stretch, but splitting a three-field struct into its own file to preserve a purity that no caller cares about is worse. Revisit if that file grows.

**Decision: no new tests are required for the move itself.**

The 345 existing tests plus #88's characterisation goldens already cover this behaviour; a pure regrouping that passes them unedited is the evidence. New tests are added only where logic moves onto a group and thereby becomes directly testable — `PickerState`'s filter/index arithmetic is the clearest case, since it previously needed a whole `App` to reach.

## Risks / Trade-offs

- **[Risk]** A mechanical rename silently changes semantics if a pattern over-matches — e.g. `.pr_target` also matching `.pr_targets`. → **Mitigation**: word-boundary-anchored patterns, and the same before/after evidence used in #88: the function inventory and the test roster must both come out byte-identical.
- **[Risk]** Flattening a reset subset while "tidying". → **Mitigation**: the three PR methods above are specified explicitly, and the existing tests cover the `pr_links`-survives case.
- **[Trade-off]** `app.pr.scroll` is one character longer to read than `app.pr_scroll` at every site. Accepted: the gain is that `PrState` can carry its own invariants and defaults, which a flat field cannot.
- **[Trade-off]** `App` still has 31 fields and a large `impl`. This change does not attempt to make `App` small — only to stop unrelated state sharing one flat namespace.
