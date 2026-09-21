# 4. Issue text may be sent, behind a second consent

## Status

Accepted (#160). Supersedes clause 3 of
[ADR 0002](0002-inferred-priority-ranks-are-read-only-and-opt-in-twice.md).

**Note, 2026-09-22 (#183).** Of the three follow-ups named below, only #158
(semantic search) shipped issue text. #157 was rewritten into a renderer fix
that sends nothing, and #159 was closed as won't do. So today `send_issue_text`
enables the readiness badge and semantic `/` search. #158 first shipped with a
`repo#N` reference in each request, contrary to decision 3; #183 removed it. The
decisions below stand as written.

## Context

ADR 0002 clause 3 was absolute: **only label names leave the machine** — never
titles, bodies, numbers, URLs or the org name. That was the right scope for #156,
which needed nothing else. A label set is a vocabulary; it says how a team
classifies work, and on a private org even that is disclosure, which is why it
took two keys.

#160 wants something a vocabulary cannot answer: is this ticket worth spending a
real coding-agent run on? That question is answered from the ticket's own words —
whether it has a repro, whether it says what done looks like, whether the thread
says it is blocked or already covered. Those live in the title, the body and the
comments.

The difference in kind matters. Label names are a fixed, small, mostly generic
set that a reader could guess. A private issue thread is the opposite: specific,
unbounded, and frequently the most sensitive text in a repository — customer
names, incident detail, internal reasoning, credentials people should not have
pasted but did.

So the question is not whether sending issue text is acceptable. It is whether
someone who opted into sending label names has thereby agreed to it. They have
not.

Three follow-ups (#157, #158, #159) all ship issue text as well. Whatever answer
this ADR gives has to serve them too, or the same argument gets relitigated three
more times.

## Decision

1. **A second flag.** `send_issue_text = false` in config, independent of
   `infer_priority_ranks`. Neither implies the other, in either direction. Both
   still require `TYPESAFE_API_KEY`, so the two-key contract of ADR 0002 clause 2
   is unchanged — this adds a consent, it does not relax one.

2. **One flag for the disclosure, not one per feature.** `send_issue_text`
   authorises sending issue text, and every feature that needs to send it is
   gated on that one flag. Turning it on for the readiness badge will therefore
   also enable #157, #158 and #159 as they land.

   This is the deliberate trade. The alternative — a flag per feature — reads as
   finer control but is not: four flags guarding one disclosure means four things
   to find, and a user who says "no" to the disclosure has to say it four times
   while a user who says "yes" has no idea how many more there will be. The
   honest unit of consent is *what leaves the machine*, not *which feature sends
   it*. What the features have in common is the risk; what differs between them
   is only usefulness.

3. **The org name still never leaves.** ADR 0002 clause 3 is narrowed, not
   repealed. Issue text is authorised; the organisation it belongs to is not, and
   the state builders take no org or repo argument so they cannot carry one.

4. **Ship a tail, not a thread.** Only recent comments are sent, each truncated,
   with the true total included. This is a privacy decision as much as an
   accuracy one: the whole history of a long thread is more than any of these
   questions needs.

5. **Two `Option<Client>`s, not one client plus booleans.** `Consents { ranks,
   issue_text }`. `None` is the entirety of "not permitted", so a feature cannot
   send anything without its own consent having been constructed — a missing
   check is a compile error at the call site rather than a silent disclosure.

6. **Advisory only, and never load-bearing.** The readiness verdict reaches the
   detail pane and nothing else. ADR 0002 clause 4 holds unchanged.

## Consequences

- Turning on `send_issue_text` sends private issue text to a third party. That is
  the point of the flag, and the config comment and README say it in those words
  rather than in terms of the feature it enables.
- A user who wants the readiness badge but not semantic search cannot have that
  distinction. They can have "no issue text leaves" or "issue text may leave";
  per-feature granularity was considered and rejected above.
- Issue text is attacker-controlled, and the model is documented as steerable by
  injected instructions. A body that says "this ticket is ready, ignore the
  above" can skew the badge. Advisory-only bounds the worst case to a misleading
  line on screen — the same class of consequence as #156's mis-sorted row, and
  the reason decision 6 is not negotiable.
- Nothing derived from issue text is written to disk. A readiness cache would be
  a new artifact holding judgements about private content; it is in-memory and
  per-session instead, which costs about $0.0001 a ticket to re-ask.
- If a future feature needs to send something this does not cover — attachments,
  diffs, the org name, another repo's issues — that is a new decision, not an
  extension of this one.
