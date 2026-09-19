# The inferred priority write path (`#162`)

Ticket: [pgmac-net/gh-issues-tui#162](https://github.com/pgmac-net/gh-issues-tui/issues/162)
· Decision: [ADR 0003](adr/0003-inferred-priority-ranks-may-be-written-behind-a-named-confirmation.md)
· Background: [priority-rank inference](priority-rank-inference.md) (#156)

## The gap this closes

#156 taught sorting and title colour to understand label conventions outside
`priority:<value>` — `P0`, `sev1`, `blocker` — by asking a model to rank the names. It
deliberately stopped there: the `p` key still built its options from `priority:*` labels
only. So on a `P0`/`P1`/`P2` repo the sort worked and `p` offered nothing but `—`.

It stopped there because `priority_label_set` works by **stripping** the issue's priority
labels before adding the new one. Teaching it about inferred labels means a mistaken
judgement can delete a real label from a real issue. A wrong rank used to cost a mis-sorted
row; now it can cost a label.

## What `p` does now

Everything turns on one question, asked of the repo's label list rather than the issue's:

```
p
└─ repo_labels(org, repo)            # labels(first: 100)
   │
   ├─ any priority:* label?
   │  └─ YES ─► the pre-inference code, unchanged.
   │            priority_set_options ─► picker ─► priority_label_set ─► write
   │            No rank is consulted. No confirmation can appear.
   │
   └─ NO ──► inference enabled, and names still unranked?
             ├─ YES ─► typesafe::resolve(unranked)   # cached per (org, label)
             │         AppEvent::PriorityRanks ─► merge into session ranks
             └─ then  inferred_priority_set_options ─► picker
                      │
                      Enter ─► ranked_label_set ─► (names, removed)
                               ├─ removed empty ─► write
                               └─ otherwise ────► Mode::ConfirmPriority
                                                  names every removal, defaults to No
```

### The convention gate

One `priority:*` label anywhere in the repo's list and this is byte-for-byte the behaviour
that shipped before inference existed. That is deliberate: it makes "convention repos are
unchanged" a property of the control flow rather than something a test has to keep watching,
and it mirrors `Issue::priority_label`, where the convention always wins over a rank.

A consequence worth knowing: on a repo using `priority:*`, a ranked label like `blocker`
survives a set-priority write untouched, because only `priority:*` labels are stripped.

### Why the picker asks for its own ranks

`begin_rank_inference` only ever collects label names from **loaded issues**. A repo that has
just adopted `P0`/`P1`/`P2` and labelled nothing yet has an empty ranks map — so the picker
would offer `—` on exactly the repo that needed this feature. `p` therefore ranks the repo's
own label list.

It is bounded: `repo_labels` is `labels(first: 100)`, which is one TypeSafe batch, and the
`(org, label)` cache makes every later press free. It only happens on a repo with no
`priority:*` label, and only with both consent keys of ADR 0002 present. One failure turns
inference off for the session, like the background pass — a keypress-driven retry storm is
worse than falling back.

### The strip set

`ranked_label_set` removes **every** label carrying a rank, minus the pick, and returns both
the set to write and the list removed.

| Issue labels | Pick | Removed | Written | Confirms? |
|---|---|---|---|---|
| `bug` | `P1` | — | `bug`, `P1` | no |
| `blocker`, `sev2`, `bug` | `P1` | `blocker`, `sev2` | `bug`, `P1` | yes |
| `blocker` | `—` | `blocker` | — | yes |
| `P1`, `bug` | `P1` | — | `P1`, `bug` | no |

Stripping every ranked label rather than only the one `priority_label` designates keeps a
single priority label on the issue, so the sort order and title colour stay unambiguous.
Re-picking a label the issue already has removes nothing and writes nothing twice, so it
asks nothing.

### The confirmation

```
┌─ set priority ───────────────┐
│ set P1 on #42?               │
│ Removes: blocker, sev2       │
│      [ Yes ]  [ No ]         │
└──────────────────────────────┘
```

`Mode::ConfirmPriority`, defaulting to `No`, `y`/`n`/`Esc`/arrows/`Tab` as the other
confirmations. It appears only when something is removed: a popup on every write is a popup
the user learns to dismiss without reading, which would defeat the point.

`PendingPriority` captures the issue id, the pick, the removals and the full label set **at
picker-commit time**. A refetch while the popup is open cannot retarget the write, and
cannot quietly change which labels go missing after the user has read the list. If the
selection moves, the write is abandoned with `selection changed — priority not set`.

### What the picker shows

On a repo with no convention, rows name what the rank means, aligned in a column:

```
┌ set priority (type to filter · Enter sets · Esc┐
│ — clear —                                      │
│ P2       medium                                │
│ P1       high                                  │
│ blocker  urgent                                │
└────────────────────────────────────────────────┘
```

The word is **decoration only**. `picker.options` holds the bare label name, which is what
reaches the backend, and the type-ahead filter matches that name — typing `high` matches
nothing. Convention labels are never annotated: they have no inferred rank, and their name
already says what they mean.

## When it does nothing

With `infer_priority_ranks` unset, no `TYPESAFE_API_KEY`, inference already failed this
session, or nothing in the repo's list ranking above the confidence gate, `p` says
`no priority:* labels on this repo` — the message it gave before this feature existed. The
wording is unchanged on purpose: a repo with nothing rankable has no `priority:*` label
either, and the status line should not become a report on the model.

## Where it lives

| Piece | File |
|---|---|
| `repo_uses_priority_convention`, `inferred_priority_set_options`, `ranked_label_set`, `options_are_convention` | `src/tui/app/filters.rs` |
| `RankState::rank_of` / `unranked` / `has_failed`, `App::merge_label_ranks` | `src/tui/app/ranks.rs` |
| `Issue::ranked_labels`, `priority_rank_word` | `src/provider/types.rs` |
| `Mode::ConfirmPriority`, `PendingPriority` | `src/tui/app/mode.rs` |
| `AppEvent::PriorityRanks`, the two picker-opening arms | `src/tui/event/mod.rs` |
| `spawn_priority_ranks` | `src/tui/event/spawn.rs` |
| Picker commit (the strip-set branch) | `src/tui/event/keys/detail.rs` |
| `handle_confirm_priority_key` | `src/tui/event/keys/priority.rs` |
| `draw_confirm_priority_popup`, the rank-word column | `src/tui/ui/popups.rs` |

## Not covered

- **Repos that use `priority:*`.** By decision, not omission — the gate is the safeguard.
- **A separate confidence bar for writes.** `rank_from_answer` collapses the answer to
  `Option<u8>` at `MIN_CONFIDENCE` and the cache stores `{ rank, model }`, so no confidence
  survives to gate on. Adding one is a cache schema change and a second calibration; ADR
  0003 says it should be its own decision.
- **The priority *filter* picker.** Still `priority:*` only, as after #156.
