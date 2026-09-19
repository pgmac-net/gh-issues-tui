# 3. Inferred priority ranks may be written, behind a named confirmation

## Status

Accepted (#162). Supersedes clause 1 of
[ADR 0002](0002-inferred-priority-ranks-are-read-only-and-opt-in-twice.md).

## Context

ADR 0002 made inferred ranks read-only. They fed `sort_issues` and `title_style`;
`priority_set_options` and `priority_label_set` kept seeing only real `priority:*` labels.
The stated cost was that on a `P0`-convention repo the `p` key still offers only `—`:
sorting works, setting a priority does not.

That ADR also said why widening the write path was held back, and what a widening would owe
an answer to. `priority_label_set` works by **stripping** the issue's priority labels before
adding the new one, so teaching it about inferred labels means a mistaken judgement removes
a real label from a real issue during a mutation. Today a wrong rank can only mis-sort a row.

Three questions had to be answered before the picker could write.

**How is a mistaken rank stopped from stripping a real label?** Three candidates: name the
removals and have the user confirm them; strip only above a much higher confidence bar than
the sort uses; or never strip, only add.

The confidence bar is not available. `rank_from_answer` collapses the answer to
`Option<u8>` at `MIN_CONFIDENCE`, and neither `RankState` nor the on-disk cache
(`Entry { rank, model }`) retains the `f64`. Threading it through means a cache schema
change and a second calibrated threshold to keep honest — and it still gives the user no
account of what is about to be deleted.

Never stripping makes the picker lie. Setting `P1` on an issue already labelled `P0` writes
`[P0, bug, P1]`; `priority_label` still returns `P0`, so the row does not move and the
title colour does not change. A silent no-op is worse than a question.

**Which label does the picker write on a repo with no `priority:*` convention?** It cannot
invent `priority:high` on a repo that uses `P1`, so it must offer the repo's own ranked
labels. But the repo's label list is not where ranks come from: `begin_rank_inference` only
ever asks about labels found on *loaded issues*. A repo that has just adopted `P0`/`P1`/`P2`
with nothing labelled yet has an empty ranks map, and the picker would have nothing to
offer — failing on exactly the repo the ticket describes.

**Does it need a confirmation naming the removals?** Yes, and that answer decides the first
question too.

## Decision

1. **The convention wins outright, and gates the whole feature.** One `priority:*` label
   anywhere in the repo's label list and the picker and the write path are the
   pre-inference code, unchanged. Inferred options appear only on repos with zero `priority:*`
   labels. This keeps "`priority_label_set` for convention repos is unchanged" true by
   construction rather than by test, and mirrors `Issue::priority_label`, where the
   convention always wins.

2. **A write that removes a ranked label names every one of them first.**
   `Mode::ConfirmPriority` lists them and defaults to `No`. A write that removes nothing
   commits straight through — there is nothing to lose, so there is nothing to ask, and a
   popup that always appears is a popup the user learns to dismiss unread.

3. **The strip set is every label carrying a rank**, minus the pick. Not just the one
   `priority_label` designates: leaving a second ranked label behind would make the issue's
   own priority a tie-break. This mirrors the convention path, which already strips every
   `priority:*` label rather than the first.

4. **The picker ranks the repo's own label list.** `p` already blocks on `repo_labels`
   (`labels(first: 100)`, one batch at most); the rank request chains onto it, and the
   per-`(org, label)` cache makes every later press free. It fires only on a repo with no
   `priority:*` label, only with both consent keys of ADR 0002 present, and a failure turns
   inference off for the session exactly as the background pass does.

5. **Clauses 2, 3 and 4 of ADR 0002 stand unchanged.** Consent still takes two keys, only
   label names leave the machine, and the judgement is still never load-bearing: with
   either key missing, offline, or on any error, `p` behaves as it did before inference
   existed, down to the status message.

6. **The rank word shown in the picker is decoration.** `picker.options` holds the label
   name that is written to the backend; the word is added at render time and the type-ahead
   filter matches the name alone. A rank word can never reach a mutation.

## Consequences

- A wrong judgement can now cause a real label to be removed — but only one the user was
  shown by name, in a popup that defaults to `No`. That is the whole of the safeguard, which
  is why the popup names the labels rather than saying "some labels".
- On a repo with no ranked label, `p` still says `no priority:* labels on this repo`. The
  wording is deliberately unchanged: a repo with nothing rankable has no `priority:*` label
  either, and the message should not turn into a report on the model.
- `p` can now make an outbound request on a keypress, where before only a data load could.
  It is bounded by the cache and by the repo's 100-label page, and gated on both consent
  keys.
- Confidence is still discarded. If a future change wants a per-decision confidence — a
  different bar for writes than for sorting — that is a cache schema change and a second
  calibration, and it should be a new ADR rather than a quiet addition to `Entry`.
- Two code paths now exist for the same keypress. They are told apart by reading the
  options the user is looking at (`options_are_convention`), not by a flag on
  `PickerState`: a flag can be left stale and disagree with the screen.
