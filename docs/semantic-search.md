# Semantic search (`#158`)

Ticket: [pgmac-net/gh-issues-tui#158](https://github.com/pgmac-net/gh-issues-tui/issues/158)
· Consent: [ADR 0004](adr/0004-issue-text-may-be-sent-behind-a-second-consent.md)

## What it does

`/` matches the query as a lowercase substring of an issue's title or body, or
its number. Searching "the auth bug" misses an issue titled *login token expiry
off-by-one*: you have to already know the author's words. And because `/` treats
the whole query as **one** needle, any multi-word natural-language query matches
almost nothing.

With semantic search on, `/` becomes a **union**:

- substring hits appear **instantly**, exactly as before;
- issues judged to be *about* the query are **added** a few seconds later.

Nothing is lost. `#123`, an exact error string and a hostname still match. A
slow, failed or offline request leaves exactly the old substring behaviour. `/`
still only **narrows** the visible set — sort order is untouched.

## Turning it on

The same two keys as the readiness badge:

```toml
# ~/.config/gh-issues/config.toml
send_issue_text = true
```

```sh
export TYPESAFE_API_KEY=...
```

Without **both**, `/` is the substring match it always was. A key exported for
other tools does not, on its own, make `/` start sending the titles and bodies
of every visible issue — including private repos — to a third party. ADR 0004
named this feature when it chose one consent flag for every feature that ships
issue text.

## What is sent

For each **candidate** — every issue that every filter *except the text* admits,
repo filter included — one request containing the query and that issue's
`repo#N`, title and first 200 characters of body. **Never the org.**

Nothing is sent for an empty query or a bare issue number (`123`, `#123`):
substring already answers those exactly.

## When it searches

After every key and every event, the app checks whether there is anything new
to ask, and asks only then:

- **the text changes** (`/`, or the filter editor's text field) — old hits are
  dropped at once, even if the new query is never sent;
- **the candidate set gains issues not yet judged for this query** — relaxing a
  filter, switching to show closed issues, or a refresh bringing new issues.

Narrowing a filter never re-sends: every remaining issue was already judged. A
response overtaken by a newer query, a cleared filter or an org switch is
dropped. One failure turns semantic search off for the session, with one status
message.

## The shape, and how it was found

**One isolated request per candidate** — state `{query, issue}`, one Noul. It is
the TypeSafe reranking cookbook's pattern: *"one request per candidate · no
request sees another"*. Getting there took a wrong turn worth recording.

The first design put every candidate in **one shared state** and asked one Noul
per candidate, naming each by a tag. It measured well once — 11/11, no false
hits — and that was **one lucky ordering**. The model has to *locate* each
candidate in a large state, and it sometimes locates the wrong one:

| referencing | failure | evidence |
|---|---|---|
| numeric tags `I000…` | "tagged **I085**" read as issue **#85** | an unrelated issue scored 0.90 on a disk query because `incidents#85` is about disks; 0.03 once the tag had no digits |
| two-letter tags `AA…` | a tag inside a word (`RL` in "URLs") | a genuine hit fell to 0.12; irrelevant issues reached 0.91 |
| paths `` `issues[17]` `` | position-dependent throughout | median order-spread 0.13–0.16 |

The test that exposed it was **order invariance** — the same issues shuffled:
a correct scheme gives each issue the same score wherever it sits. It measures
the mechanism, not the answers, so it cannot be fitted. With one issue per
request there is no position at all.

**Cost of the shape:** ~150 parallel requests per search (capped by
`MAX_IN_FLIGHT`), about **4–6 seconds** before semantic hits land, and roughly
61k input tokens — **about $0.003** a search. 141 parallel requests saw no rate
limiting.

## The threshold — an override, stated plainly

`SEARCH_YES = 0.70`. **It is not what the measurement produced.**

A rule was fixed and committed **before** the measurement (`c3ce53c`): the
midpoint of the gap between the best irrelevant score and the worst genuine hit
across all fourteen runs — and an **empty gap ships no threshold**. The gap was
empty.

Measured over the nine public `pgmac-net` repos with issues (141 issues), seven
queries whose expected hits were written and hashed before any request, each run
over the full corpus and again scoped to one repo:

| query | run | candidates | worst genuine hit | best irrelevant | noise (median / max) |
|---|---|---|---|---|---|
| persistent disks becoming unwritable in kubernetes | full | 141 | 0.71 | 0.66 | 0.01 / 0.09 |
|  | scoped | 13 | 0.80 | 0.61 |  |
| traffic blackholed by a leftover routing entry | full | 141 | 0.77 | 0.13 | 0.00 / 0.01 |
|  | scoped | 13 | 0.77 | 0.08 |  |
| keeping secrets out of version control | full | 141 | 0.97 | 0.64 | 0.00 / 0.03 |
|  | scoped | 9 | 0.96 | 0.05 |  |
| opening web addresses by pointing at them | full | 141 | 0.94 | 0.60 | 0.00 / 0.04 |
|  | scoped | 63 | 0.94 | 0.14 |  |
| choosing how urgent a ticket is | full | 141 | 0.80 | **0.84** | 0.00 / 0.08 |
|  | scoped | 63 | 0.83 | 0.63 |  |
| putting text somewhere it can be pasted later | full | 141 | 0.92 | 0.48 | 0.00 / 0.07 |
|  | scoped | 36 | 0.91 | 0.13 |  |
| a cooking recipe for sourdough bread | full | 141 | — | 0.02 | 0.00 / 0.01 |
|  | scoped | 63 | — | 0.02 |  |

"Noise" is the same issue scored twice (full and scoped run): with one issue per
request nothing else differs, so it is pure model variation — and it is tiny.

The one issue that empties the gap is **`incidents#85` at 0.84** on "choosing how
urgent a ticket is". It is an outage report that opens *"Provisional severity: P2
… Confirmed P2"* — it records a severity choice. That is ambiguity of meaning,
not a mechanical fault.

0.70 was then chosen as a **product decision, with that data**: every genuine
hit clears it in both runs (11/11), and across ~1,250 judgements the only false
hit is that report. Semantic search only ever *adds* rows to a union, so a stray
related row costs little, and a miss costs little too because substring still
works.

**Fragile at the bottom:** the worst genuine hit, `incidents#82`, scored 0.71 in
one run and 0.80 in the other, so it can flicker near the line.

The offline guards pin all of this: that the rule could **not** produce a
threshold (a future clean gap fails the guard and says to use the rule instead),
that every genuine hit clears 0.70, and that the **only** false hit is exactly
that report — so a re-record that adds another fails loudly.

## Re-recording

```sh
cargo test calibrate_semantic_search_against_live_api -- --ignored --nocapture
```

Needs `TYPESAFE_API_KEY` and a GitHub token; public repos only; about two cents;
reports and asserts nothing, so model drift cannot fail CI. It panics if the
corpus is no longer exactly the 141 issues measured. **Read the table before
touching `SEARCH_YES`**, and change the question, state shape or model only
together with a re-record — the digest guard fails otherwise.

## Not verified

- **Public issues only.** Private repos — where your own wording drifts most, and
  where semantic search would matter most — were deliberately not measured,
  because measuring them would have sent their text off the machine.
- **Seven queries, eleven genuine hits.** A bound, not a calibration.
- Not driven end-to-end in the running app: the union, triggers and stale
  handling are covered by unit tests, the request by the live harness.
