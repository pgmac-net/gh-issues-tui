# 2. Inferred priority ranks are read-only, and opt-in twice

## Status

Accepted (#156)

## Context

`Issue::priority_rank` only understands the `priority:<value>` label convention, so a repo
labelling priority `P0`/`sev1`/`blocker` ranks every issue 0 and the priority sort silently
does nothing. #156 fixes that by asking a language model to rate unfamiliar label names.

That is the first time this app sends data anywhere but the issue backend, and the first
time a model's judgement influences what the user sees. Two questions had to be answered,
and both are easy to get wrong later by accident.

**Where may a judgement reach?** The rank is needed by sorting and colouring. It could
also reach the set-priority picker and `priority_label_set`, which builds the label array
written back to the backend — that would make `p` work on any convention. But
`priority_label_set` works by *stripping* priority labels before adding the new one, so
teaching it about inferred labels means a mistaken judgement removes a real label from a
real issue during a mutation.

**What counts as consent?** The only credential needed is `TYPESAFE_API_KEY`. That is a
general-purpose key plausibly exported in a shell profile for unrelated tools. Label names
are org data: `customer-acme-escalation` is a real shape, and on a private org the names
alone say something.

## Decision

1. **Inferred ranks are read-only.** They feed `sort_issues` and `title_style`, and nothing
   that writes. `priority_set_options` and `priority_label_set` keep seeing only real
   `priority:*` labels.
2. **Consent takes two keys.** `infer_priority_ranks = true` in config *and*
   `TYPESAFE_API_KEY` in the environment. The environment variable is the credential; the
   config flag is this app's consent to send label names. A key exported for another tool
   does not, by itself, make this one start making outbound calls.
3. **Only label names leave the machine** — never titles, bodies, numbers, URLs or the org
   name.
4. **The judgement is never load-bearing.** Any error, or either key missing, falls back to
   the pre-existing behaviour exactly.

## Consequences

- A wrong judgement can mis-sort a row. It cannot change what is stored on a backend, and
  a prompt-injected label name can at worst mis-rank that one label.
- On a `P0`-convention repo the `p` picker still offers only `—`. That is a deliberate
  cost, not an oversight.
- Widening the write path is a new decision, not a tidy-up. It needs its own answer to how
  a mistaken rank is prevented from stripping a real label, and should not be reached by
  extending `priority_label_set` because it looks like the obvious next step.
- Setting up the feature is two steps. That friction is the point: it is what stops a key
  exported for another purpose silently enabling outbound calls here.
- `TYPESAFE_API_KEY` must stay out of `Config`, consistent with the existing rule that
  tokens are never stored in config.
