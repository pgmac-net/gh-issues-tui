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
readiness: thin — no specifics, criteria
readiness: blocked — waiting on something unresolved
readiness: ready — is specific and states an outcome
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
| `specifics` | **Could someone begin work on this without having to ask what is meant?** Names what to change, where, or how to see the current behaviour | quality |
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
specifics, criteria, blocked or duplicate undecided -> "unsure — cannot judge <which>"
otherwise, from specifics and criteria:
  both present -> "ready"
  either absent -> "thin — no <which>"
```

**The verdict reads every signal one-sidedly, and only the signals a claim rests on
earn an `unsure`.** `actionable` is consumed only as `< 0.3`, so a middling value
just means *not vetoed*; hedging on it makes a well-specified bug report read
`unsure` for a property nothing depends on (#169).

| signal | direction read | hedges when in 0.3–0.7? | why |
|---|---|---|---|
| `specifics`, `criteria` | `< NO` → thin | yes | "ready — is specific and states an outcome" is a positive claim; it must not be asserted on a coin flip |
| `blocked`, `duplicate` | `> YES` → veto | yes | missing one costs a whole agent run, so "might be" is worth saying |
| `actionable` | `< NO` → veto | **no** | only its low end matters, and real tickets sit in its band routinely |

This is the `HEDGED` constant in `readiness.rs`, written as a rule rather than a
list: a signal belongs there only if the verdict makes a claim resting on it being
decisive. Add a signal and you must decide which direction the verdict reads it.
A single weighted score would have averaged a veto away: a blocked ticket with an
excellent specifics and clear criteria reads as ready. The raw five probabilities are
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
| `specifics` | 14 | 5 | 0.59 | 0.48 | **no, by 0.11** — but see below |
| `criteria` | 13 | 5 | 0.45 | 0.60 | yes |
| `blocked` | 0 | 22 | 0.43 | — | `yes` side **unmeasured** |
| `duplicate` | 2 | 18 | 0.54 | 0.86 | **yes** — corrected in #170, see below |

`actionable` is deliberately not in that table. It is read only as `< NO`, so it has
no yes-side to separate: 18 cases are asserted *not vetoed* and 2 asserted vetoed,
and `NO` must fall in **(0.13, 0.34]** — above the worst vetoed case and no higher
than the worst real work item. `NO = 0.3` does.

Probabilities drift between runs. The same wordings and model, re-recorded for
#169, moved by up to 0.22; that was all on `gh-issues-tui#168`, a live ticket
whose thread had grown in between, so it is real input change rather than
necessarily model noise — but the two cannot be cleanly separated, so treat
differences of ~0.1–0.2 as within noise. Re-recorded again for #170 on unchanged
wordings, drift was at most 0.06.

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

**`specifics` replaced `repro` (#171), and improved but did not clear the bar.**
`repro` asked whether someone "could see the problem or the current behaviour for
themselves". That asks about the *current* state, so a precise spec for something
not yet built scored 0.08 while a record of merged work scored 0.73 — right for the
question, wrong for readiness. It also correlated with `criteria` at r = 0.81, and
every divergence was a failure case.

`specifics` asks the decision the badge serves — *could someone begin work on this
without having to ask what is meant?* — and names the three forms specifics take
(what to change, where, or how to see current behaviour), so a defect report and a
feature spec can both satisfy it. Measured:

- `Docker-Nagios#4`, a precise spec, no longer reads `thin`.
- The three cases where `repro` was the sole reason for `unsure` all resolved:
  `incidents#48` 0.59→0.82 and `#75` 0.36→0.78 read `ready`; `#49` 0.40→0.77 now
  reads `thin — no criteria`, correctly, for an investigation with no definition of done.
- **It still does not separate**: worst no 0.59, worst yes 0.48, inverted by 0.11
  (was 0.33 for `repro`).

**That inversion rests entirely on two cases**, both flagged as hard before any
measurement: `incidents#72` (asserted `no`; names concrete wants but says it needs
brainstorming — the one expectation I hesitated on) and `Docker-Nagios#4` (asserted
`yes`; the anchor case). Set those two aside and the rest separate at **0.28 .. 0.74**.

Neither was re-marked after the result. Moving `incidents#72` to unasserted would
make the gap positive, which is exactly why it was not done: the expectations were
committed before the measurement so that this could not be quietly adjusted.
`specifics` is therefore a better signal than `repro` that is still not a threshold
one can defend.

**`duplicate` works, and #168 wrongly reported that it did not (#170).** The veto
fires on exactly the two real duplicates — `incidents#86` ("Addressed in #87
(merged)", still open) and `nagios-public-status-page#71` ("Already fixed by 5e6e6e9
(PR #70, merged)") — and on nothing else. The gap is 0.54 .. 0.86, and `YES = 0.7`
sits inside it.

The original finding was mine and it was wrong, in the same way as `actionable`'s but
for a different reason. #168 marked `nagios-public-status-page#71` as *not* a
duplicate and reported its 0.97 as a false positive — "a citation is being read as a
coverage claim". It is not a citation. Its only comment says it was already fixed,
and `#67`'s merge comment independently says it fixed `#71` along the way. **I judged
the ticket from its body and never read the thread**; the model read the thread and
was right. Correcting that one expectation takes the gap from −0.10 to +0.32.

That is a change to an expectation after seeing the result, so it has to be
defensible: it is a quoted sentence I demonstrably did not read, not a re-judgement.
`docker-registry-walk#59` (0.54, asserted `no`) is now the case that sets the no-end,
and is left alone — its body says "gh-issues-tui already has a mature version of
this", which is arguable but not factually wrong, and re-marking it would be the
re-judgement this corpus does not allow. It reads `unsure — cannot judge duplicate`,
a false hedge rather than a false veto.

What #168 got right, and stands: the **round-2 rewording** ("has the work already
been done … nothing left to do here") really did produce six false positives, because
a closed thread ends in its own "Work complete" and that wording is true of any
finished ticket. It was reverted. The claim that the *current* wording has that
problem is what was wrong.

**`blocked` was fixed during calibration.** As shipped in #167 it fired on seven
tickets with no blocker at all, up to 0.70, because "waiting on … a decision" is
true of any vague ticket. It now names an *external* dependency and says outright
that vague, undesigned, unscheduled or under-investigation is not blocked. All 22
cases now score at most 0.43.

**`criteria` is the one signal that works** as intended.

### Consequence for the badge

`unsure` on 7 of 22 cases (14 before #169, 9 before #171, 8 before #170), with 8
`ready` (from 1) and the five vetoes unchanged. It is more useful than it was, but it
still declines to speak on 7 tickets, and `specifics` does not separate. **Treat it as unproven until the follow-ups land:**

| finding | ticket |
|---|---|
| ~~`actionable` under-reads real work~~ — the veto was right; the undecided check was two-sided. Fixed | [#169](https://github.com/pgmac-net/gh-issues-tui/issues/169) |
| ~~`duplicate` conflates "covered elsewhere" with "finished"~~ — it works; the expectation was wrong, not the question. Corrected | [#170](https://github.com/pgmac-net/gh-issues-tui/issues/170) |
| ~~`repro` asks two questions at once~~ — replaced by `specifics`, which improved but is still inverted by 0.11 on two contested cases | [#171](https://github.com/pgmac-net/gh-issues-tui/issues/171) |
| the corpus cannot measure `blocked=yes` or `duplicate=yes` | [#172](https://github.com/pgmac-net/gh-issues-tui/issues/172) |

### The corpus, and its limits

Public repos only, so any reviewer can open any case and disagree with the
expectation recorded against it. Expectations were written from reading each
ticket before any request, and are never edited to make a number pass — only
wordings change, and every change is disclosed. **One exception, and it is the
instructive one:** `nagios-public-status-page#71`'s `duplicate` expectation was
corrected (#170) because I had judged it from the body without reading its comment.
That is allowed only when the change is a fact you can quote, and never a
re-judgement. **Judge an expectation against the whole thread, not the body** — the
error was in reading, and it survived two rounds of measurement because every later
number was compared to it.

- **`blocked = yes` is unmeasured.** No open issue in any public `pgmac-net` repo
  is waiting on something unresolved as its thread currently stands. None was
  manufactured.
- **`duplicate = yes` rests on two cases** (`incidents#86`, still open, and
  `nagios-public-status-page#71`, closed). Better than one, still thin: a gap of 0.32
  measured from two positives is a bound, not a calibration.
- **The corpus is weighted to closed tickets**, which is out of domain — the badge
  exists to be read *before* starting work. `homelabia` has the variety and 194
  issues, but is private, so its tickets cannot carry committed expectations in a
  public repo.

### The recording

| ref | specifics | criteria | actionable | blocked | duplicate | verdict |
|---|---|---|---|---|---|---|
| `nagios-public-status-page#69` | 0.88 | 0.85 | 0.63 | 0.04 | 0.23 | ready |
| `nagios-public-status-page#60` | 0.95 | 0.90 | 0.50 | 0.13 | 0.25 | ready |
| `nagios-public-status-page#67` | 0.94 | 0.91 | 0.57 | 0.08 | 0.18 | ready |
| `nagios-public-status-page#71` | 0.94 | 0.94 | 0.55 | 0.03 | 0.97 | may be a duplicate |
| `incidents#48` | 0.83 | 0.91 | 0.94 | 0.07 | 0.03 | ready |
| `docker-registry-walk#59` | 0.93 | 0.91 | 0.54 | 0.07 | 0.54 ! | unsure |
| `docker-registry-walk#96` | 0.81 | 0.88 | 0.46 | 0.43 ! | 0.12 | unsure |
| `incidents#86` | 0.93 | 0.88 · | 0.46 | 0.04 | 0.86 | may be a duplicate |
| `incidents#49` | 0.77 | 0.23 | 0.96 | 0.13 | 0.06 | thin |
| `Docker-Nagios#1` | 0.28 | 0.45 ! | 0.87 | 0.08 | 0.03 | unsure |
| `incidents#72` | 0.59 ! | 0.60 · | 0.69 | 0.11 | 0.05 | unsure |
| `gh-issues-tui#60` | 0.16 | 0.13 | 0.83 · | 0.12 | 0.04 | thin |
| `Docker-Nagios#3` | 0.12 | 0.06 | 0.16 · | 0.10 | 0.32 · | not a work item |
| `Docker-Nagios#4` | 0.48 ! | 0.82 | 0.94 | 0.12 | 0.03 | unsure |
| `metasearch#22` | 0.42 · | 0.60 ! | 0.50 | 0.07 | 0.05 | unsure |
| `metasearch#19` | 0.67 · | 0.77 · | 0.34 | 0.04 | 0.17 | unsure |
| `gh-issues-tui#129` | 0.86 | 0.85 | 0.64 | 0.07 | 0.04 | ready |
| `gh-issues-tui#130` | 0.11 | 0.08 | 0.13 | 0.19 | 0.03 | not a work item |
| `tremendous-cve#10` | 0.80 · | 0.71 · | 0.06 | 0.06 | 0.30 · | not a work item |
| `incidents#75` | 0.78 | 0.77 | 0.97 | 0.07 | 0.27 | ready |
| `gh-issues-tui#160` | 0.83 | 0.91 | 0.86 | 0.09 | 0.08 | ready |
| `gh-issues-tui#168` | 0.74 | 0.86 | 0.65 | 0.28 | 0.10 | ready |

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
