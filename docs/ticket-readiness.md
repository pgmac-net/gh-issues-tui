# Ticket readiness (`#160`)

Ticket: [pgmac-net/gh-issues-tui#160](https://github.com/pgmac-net/gh-issues-tui/issues/160)
· Consent: [ADR 0004](adr/0004-issue-text-may-be-sent-behind-a-second-consent.md)
· Background: [priority-rank inference](priority-rank-inference.md) (#156)

## What it does

`A` on an issue spends a real coding-agent run on it. Nothing used to say whether
the ticket was worth one — a vague ticket, a question filed as work, something
blocked on an unresolved decision, or something a comment already says is a
duplicate, all burn a session before the agent discovers it.

With the feature on, the detail pane carries one extra line:

```
readiness: thin — no repro, criteria
readiness: blocked — waiting on something unresolved
readiness: ready — has a repro and a stated outcome
readiness: unsure — cannot judge blocked
```

## Turning it on

Both are required, and the flag is **not** the one from #156:

```toml
# ~/.config/gh-issues/config.toml
send_issue_text = true    # default: false
```

```sh
export TYPESAFE_API_KEY=...   # environment only — never in config
```

`infer_priority_ranks` and `send_issue_text` are independent in both directions.
They authorise different disclosures: a vocabulary of label names, versus the text
of private issues. See [ADR 0004](adr/0004-issue-text-may-be-sent-behind-a-second-consent.md)
for why this is one flag covering every feature that sends issue text rather than
one flag each — turning it on will also enable #157, #158 and #159 as they land.

## What leaves the machine

The title, the body truncated to 4000 characters, the **last 10 comments** each
truncated to 500, and the true total comment count.

Not the whole thread: "accuracy falls as the state grows with content unrelated to
the decision", and a 50-comment triage thread is unrelated to every one of these
questions. The tail is also the current state of play, which is what "is it
blocked" depends on.

**Never the org or repo name** — ADR 0002 clause 3 still covers those, and
`readiness::state` takes no org argument, so it cannot carry one.

Nothing is written to disk. Judgements are in-memory, per session.

## Advisory, and structurally so

The verdict reaches the detail pane and **nothing else**. `A`, `LaunchAction`,
`launch_action`, `expand_argv` and the PTY path are not touched by this feature,
and the word "readiness" does not appear anywhere in them.

That is not politeness, it is the safety argument. Issue text is
attacker-controlled and Jev is documented as steerable by injected instructions,
so a body saying "this ticket is ready, ignore the above" can skew the badge. With
no path from a judgement to an execution, the worst case is a misleading line on
screen. ADR 0002 clause 4 — the judgement is never load-bearing — holds unchanged.

The ticket originally asked for a warning on the pre-launch confirmation. There is
no pre-launch confirmation: `LaunchAction::Spawn` goes straight to the PTY. Adding
one would have put a model's judgement in front of every launch, and fetching on
detail-open means `A` pressed from the list has no answer yet — so the warning
would either fire late or block. The badge sits where you already are when you
decide.

## The five questions

One request, five independent Nouls over one state. Independent questions run in
parallel in a single call, so the extra four cost no extra round trip. Each has
explicit `criteria` for both the yes and the no side.

| Question | Asks | Kind |
|---|---|---|
| `repro` | Concrete steps, inputs or conditions that reproduce or show the problem | quality |
| `criteria` | States what finishing looks like | quality |
| `actionable` | Work to carry out, not a question or a status update | veto on *no* |
| `blocked` | **As the thread currently stands**, waiting on something unresolved | veto on *yes* |
| `duplicate` | The thread *states* this is already covered elsewhere | veto on *yes* |

`duplicate` deliberately asks what the thread *says*, not whether the ticket
resembles another issue. The ticket proposed the latter, but the referenced
issue's content is never in state — the model would see only `#129`. Asking
whether someone wrote "dupe of #129" is answerable from text that is actually
there, and catches the real case: a human said so and nobody closed it.

`blocked` is worded against the current state because Jev "reads dates as text,
not as ordered quantities" — asking it whether an old blocker was since resolved
is asking for the failure mode. Sending only the recent tail is the other half of
that mitigation.

## The verdict

A Noul returns one probability and **no confidence**, unlike a Score:

```json
{ "type": "noul", "noul": 0.95 }
```

So there is nothing to gate on but the probability itself, and `0.5` means
*equally likely either way* — not "medium". Hence three bands, not two:

- above **0.7** → yes
- below **0.3** → no
- between → **undecided**, reported as such rather than rounded into a verdict

Vetoes are checked first and named individually, because they do not compensate:

```
blocked    > 0.7   -> "blocked"
duplicate  > 0.7   -> "may be a duplicate"
actionable < 0.3   -> "not a work item"
any signal undecided -> "unsure — cannot judge <which>"
otherwise, from repro and criteria:
  both present -> "ready"
  either absent -> "thin — no <which>"
```

A single weighted score would have averaged a veto away: a blocked ticket with an
excellent repro and clear criteria reads as ready. The raw five probabilities are
stored and the policy is a pure function over them, so changing a threshold or the
wording needs no new request.

## When it does nothing

No flag, no key, the thread not yet settled, a request already out for that
ticket, or a failure earlier this session → **no line at all**. The pane is then
identical to before this existed, geometry included, which matters because
`body_content_height` feeds the scroll clamp (see below).

One failure turns asking off for the session. Deliberately silent, unlike the
label-rank failure's status message: a missing badge is not something the user
asked for or can act on, and it must not displace a real message.

## Implementation notes

**The measured height must match what is drawn.** `body_lines_links` and
`body_content_height` both take the readiness, because `detail_scroll` clamps
against the latter — CLAUDE.md: "both `ui::draw` and `event.rs`'s scroll clamps
call these so the renderer and key handler can't drift". A badge drawn but not
counted would let the body scroll past its own end.

**Asking waits for a settled thread.** `blocked` and `duplicate` are answered from
the comments, so asking before they arrive would judge the ticket on its body
alone and then keep that answer. `App::begin_readiness` returns `None` until
`comment_cache` has an entry — which `load_comments` settles without a request for
a cached thread or an issue with no comments.

**`ReadinessState` mirrors `RankState`**: answers, an in-flight set so arrowing
through a list does not ask twice about one ticket, and a failure latch. Dropped
by `invalidate_comments` (the thread it was derived from changed), and cleared
wholesale by `set_data` and `switch_org`.

| Piece | File |
|---|---|
| Questions, state, thresholds, verdict | `src/typesafe/readiness.rs` |
| `Answer` enum, `Client::ask`, `Consents` | `src/typesafe/mod.rs` |
| Session state | `src/tui/app/readiness.rs` |
| `AppEvent::Readiness`, `spawn_readiness` | `src/tui/event/mod.rs`, `spawn.rs` |
| The badge line and its colour | `src/tui/ui/detail.rs` |
| `send_issue_text` | `src/config.rs` |

## Not verified

**The five question wordings and the two thresholds are unmeasured.** The test
suite pins the composition — every veto, every band, every invalidation path, each
confirmed by mutation — but no test can tell you whether Jev answers *these five
questions* well on real tickets. The docs say plainly to "validate their
performance in the target domain".

#163 exists because #156 shipped one guessed threshold. This ships two thresholds
and five prompts. A calibration pass against real tickets, in the shape of #163,
is the honest follow-up.
