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
repro, criteria, blocked or duplicate undecided -> "unsure — cannot judge <which>"
otherwise, from repro and criteria:
  both present -> "ready"
  either absent -> "thin — no <which>"
```

**The verdict reads every signal one-sidedly, and only the signals a claim rests on
earn an `unsure`.** `actionable` is consumed only as `< 0.3`, so a middling value
just means *not vetoed*; hedging on it makes a well-specified bug report read
`unsure` for a property nothing depends on (#169).

| signal | direction read | hedges when in 0.3–0.7? | why |
|---|---|---|---|
| `repro`, `criteria` | `< NO` → thin | yes | "ready — has a repro and a stated outcome" is a positive claim; it must not be asserted on a coin flip |
| `blocked`, `duplicate` | `> YES` → veto | yes | missing one costs a whole agent run, so "might be" is worth saying |
| `actionable` | `< NO` → veto | **no** | only its low end matters, and real tickets sit in its band routinely |

This is the `HEDGED` constant in `readiness.rs`, written as a rule rather than a
list: a signal belongs there only if the verdict makes a claim resting on it being
decisive. Add a signal and you must decide which direction the verdict reads it.
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

## What calibration showed (#168)

Measured against 22 real, public `pgmac-net` tickets. **The result is that the
feature does not work as intended, and the thresholds cannot yet be justified.**

`YES = 0.7` and `NO = 0.3` were deliberately **not changed**: no single pair fits
all five signals, so moving them would hide the problem rather than fix it.

### Per-signal separation

For each signal, the worst case a reader marked `no` against the worst marked
`yes`. A threshold can only exist between them.

| signal | n(yes) | n(no) | worst no | worst yes | separates? |
|---|---|---|---|---|---|
| `repro` | 11 | 9 | 0.73 | 0.40 | **no** |
| `criteria` | 13 | 5 | 0.43 | 0.60 | yes |
| `blocked` | 0 | 22 | 0.47 | — | `yes` side **unmeasured** |
| `duplicate` | 1 | 19 | 0.97 | 0.84 | **no** |

`actionable` is deliberately not in that table. It is read only as `< NO`, so it has
no yes-side to separate: 18 cases are asserted *not vetoed* and 2 asserted vetoed,
and `NO` must fall in **(0.13, 0.34]** — above the worst vetoed case and no higher
than the worst real work item. `NO = 0.3` does.

Probabilities drift between runs. The same wordings and model, re-recorded for
#169, moved by up to 0.22; that was all on `gh-issues-tui#168`, a live ticket
whose thread had grown in between, so it is real input change rather than
necessarily model noise — but the two cannot be cleanly separated, so treat
differences of ~0.1–0.2 as within noise.
### What that means for each question

**`actionable` works, and #168 wrongly reported that it did not.** The veto fires
on exactly three cases and all three are right — `gh-issues-tui#130` (literally a
question), `Docker-Nagios#3` (115 characters, "Nothing to see here"),
`tremendous-cve#10` (a record of merged work) — and never on a real work item.

The original finding was mine and it was wrong. I marked `actionable: yes` on 18
cases, a two-sided expectation for a signal the verdict reads one-sidedly, then
reported the low yes-side (0.34–0.55, against a `YES` of 0.7) as the question
under-reading. Nothing consumes that side. The real defect was in the composition:
the undecided check demanded every signal sit outside 0.3–0.7, so a middling
`actionable` made five well-specified bug reports read `unsure`. That is fixed by
scoping the hedge, not by rewording the question — see the table above.

**`repro` asks two questions at once** — "is there a reproduction" and "is this
specific enough to act on" — and a feature request cannot have the first. Two
corpus cases had to be left unasserted for exactly that reason. No wording fixes
this; the signal is doing two jobs.

**`duplicate` conflates "covered somewhere else" with "this ticket is finished".**
A closed thread ends in its own "Work complete", so any wording asking whether the
work is already done reads as true. A rewording during calibration made this
worse — six false positives against one true positive — and was reverted. It needs
redesign, not rewording.

**`blocked` was fixed during calibration.** As shipped in #167 it fired on seven
tickets with no blocker at all, up to 0.70, because "waiting on … a decision" is
true of any vague ticket. It now names an *external* dependency and says outright
that vague, undesigned, unscheduled or under-investigation is not blocked. All 22
cases now score at most 0.47.

**`criteria` is the one signal that works** as intended.

### Consequence for the badge

`unsure` on 9 of 22 cases, down from 14 before #169 scoped the hedge, with 6 `ready`
(from 1) and the five vetoes unchanged. It is more useful than it was, but it still
declines to speak on 9 tickets, and `repro` and `duplicate` genuinely do not
separate. **Treat it as unproven until the follow-ups land:**

| finding | ticket |
|---|---|
| ~~`actionable` under-reads real work~~ — the veto was right; the undecided check was two-sided. Fixed | [#169](https://github.com/pgmac-net/gh-issues-tui/issues/169) |
| `duplicate` conflates "covered elsewhere" with "finished" | [#170](https://github.com/pgmac-net/gh-issues-tui/issues/170) |
| `repro` asks two questions at once | [#171](https://github.com/pgmac-net/gh-issues-tui/issues/171) |
| the corpus cannot measure `blocked=yes` or `duplicate=yes` | [#172](https://github.com/pgmac-net/gh-issues-tui/issues/172) |

### The corpus, and its limits

Public repos only, so any reviewer can open any case and disagree with the
expectation recorded against it. Expectations were written from reading each
ticket before any request, and are never edited to make a number pass — only
wordings change, and every change is disclosed.

- **`blocked = yes` is unmeasured.** No open issue in any public `pgmac-net` repo
  is waiting on something unresolved as its thread currently stands. None was
  manufactured.
- **`duplicate = yes` rests on one case** (`incidents#86`: "Addressed in #87
  (merged)", still open). One case cannot measure a signal.
- **The corpus is weighted to closed tickets**, which is out of domain — the badge
  exists to be read *before* starting work. `homelabia` has the variety and 194
  issues, but is private, so its tickets cannot carry committed expectations in a
  public repo.

### The recording

| ref | repro | criteria | actionable | blocked | duplicate | verdict |
|---|---|---|---|---|---|---|
| `nagios-public-status-page#69` | 0.92 | 0.84 | 0.64 | 0.03 | 0.23 | ready |
| `nagios-public-status-page#60` | 0.97 | 0.89 | 0.53 | 0.13 | 0.25 | ready |
| `nagios-public-status-page#67` | 0.94 | 0.91 | 0.59 | 0.08 | 0.21 | ready |
| `nagios-public-status-page#71` | 0.96 | 0.94 | 0.54 | 0.03 | 0.97 ! | may be a duplicate |
| `incidents#48` | 0.59 ! | 0.91 | 0.95 | 0.07 | 0.03 | unsure |
| `docker-registry-walk#59` | 0.87 | 0.91 | 0.51 | 0.06 | 0.49 ! | unsure |
| `docker-registry-walk#96` | 0.87 | 0.87 | 0.45 | 0.47 ! | 0.12 | unsure |
| `incidents#86` | 0.95 | 0.88 · | 0.46 | 0.05 | 0.84 | may be a duplicate |
| `incidents#49` | 0.40 ! | 0.23 | 0.97 | 0.13 | 0.06 | unsure |
| `Docker-Nagios#1` | 0.08 | 0.43 ! | 0.87 | 0.09 | 0.03 | unsure |
| `incidents#72` | 0.41 ! | 0.61 · | 0.71 | 0.10 | 0.06 | unsure |
| `gh-issues-tui#60` | 0.07 | 0.12 | 0.81 · | 0.12 | 0.04 | thin |
| `Docker-Nagios#3` | 0.04 | 0.06 | 0.14 · | 0.10 | 0.39 · | not a work item |
| `Docker-Nagios#4` | 0.08 | 0.81 | 0.95 | 0.12 | 0.03 | thin |
| `metasearch#22` | 0.45 ! | 0.60 ! | 0.49 | 0.06 | 0.04 | unsure |
| `metasearch#19` | 0.82 | 0.76 · | 0.34 | 0.04 | 0.15 | ready |
| `gh-issues-tui#129` | 0.85 | 0.86 | 0.63 | 0.06 | 0.04 | ready |
| `gh-issues-tui#130` | 0.04 | 0.08 | 0.13 | 0.19 | 0.04 | not a work item |
| `tremendous-cve#10` | 0.73 ! | 0.70 · | 0.06 | 0.06 | 0.27 · | not a work item |
| `incidents#75` | 0.36 ! | 0.78 | 0.97 | 0.07 | 0.28 | unsure |
| `gh-issues-tui#160` | 0.78 · | 0.91 | 0.86 | 0.09 | 0.08 | ready |
| `gh-issues-tui#168` | 0.76 · | 0.86 | 0.67 | 0.31 ! | 0.10 | unsure |

`!` disagrees with the expectation · `·` unasserted

Full numbers in `src/typesafe/readiness-calibration.json`. Re-run with:

```sh
cargo test calibrate_readiness_against_live_api -- --ignored --nocapture
```

Needs `TYPESAFE_API_KEY` and a GitHub token, costs about a third of a cent, and
overwrites the recording. It reports only and asserts nothing, so model drift
cannot fail CI. Offline tests then hold the code to the recording: they pin which
signals separate and which do not, and a reworded question fails the digest guard
rather than silently inheriting this tuning. **Read the table before touching
`YES` or `NO`.**
