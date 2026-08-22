# PR URL matching (`#129`)

Ticket: [pgmac-net/gh-issues-tui#129](https://github.com/pgmac-net/gh-issues-tui/issues/129)

## What it does

`P` used to find only explicit `github.com/{owner}/{repo}/pull/{N}` links. GitHub
renders cross-repo references in a shortened form — `pgmac-net/gh-issues-tui#72`
— and people type bare `#72` constantly, so `P` reported "no PR links found" on
threads that were full of them.

It now recognises three forms:

| Form | Example | Repo comes from |
|---|---|---|
| Explicit link | `github.com/o/r/pull/72` | the URL |
| Qualified shorthand | `o/r#72` | the reference |
| Bare shorthand | `#72` | the thread's own repo |

All three collapse into one `PrRef` list, in first-seen order, deduped — so a
comment carrying both a link and its shorthand yields one candidate, not two.

## The ambiguity, and why resolution is lazy

GitHub allocates pull request and issue numbers from **one sequence per repo**.
That is what makes the shorthand usable at all — `o/r#72` names exactly one
object — but it also means the text cannot say which kind it names. Nothing in
`#72` distinguishes an issue from a PR.

So a parsed reference is a **candidate**. Its type is settled by fetching it,
and `Client::pull_request` returns:

```rust
pub enum PrLookup {
    Pr(Box<PrSummary>),
    Issue(IssueRef),
}
```

The issue case is a normal outcome, not an error. Only a number that is
*neither* — deleted, or never existed — is an error, and it keeps the old
`no such PR o/r#N` message.

This is done in the existing `PR_SUMMARY_QUERY` by asking for both:

```graphql
pullRequest(number: $number) { … }
issue(number: $number) { number title url }
```

`issue` is a single node, not a connection, so per `docs/graphql-api-cost.md` it
adds **no points** — the cost law only charges for nested connections. One
request still answers the whole question.

**Rejected: resolving candidates before the picker opens.** It would let the
picker show only real PRs, but at one request per candidate, paid every time `P`
is pressed, and with a loading state on a key that is currently instant.

## What happens on an issue

The popup shows the issue's number and title with
`issue, not a pull request — o/Enter jumps to it`.

`o`/Enter then tries to move the selector to that issue, via
`App::jump_to_ref(Some(repo), number)`. That is a repo-pinned variant of the
existing `#`-jump: numbers repeat across repos, so following `o/r#7` must not
land on some other repo's `#7`, which is exactly what the unpinned
`jump_to_number` would do when the selection already sat elsewhere.

A jump is only possible when the reference is inside the loaded data — the
owner matches `App::org`, the repo is loaded, and the issue itself was fetched
(closed issues need `f` or `--all`). Otherwise `o`/Enter **opens the issue in a
browser** instead, using the URL the same query already returned.

`jump_to_ref` deliberately reports failure *without* setting `status`, unlike
`jump_to_number`. The caller is the one that knows whether giving up means "no
issue #N loaded" or "opened <url>", so the message belongs to it.

## Keeping false positives down

Bare `#N` is the noisiest pattern in issue prose, so the scanner applies three
rules:

1. **Boundary before.** A shorthand must start the text or follow whitespace or
   one of `([{<"',;:|*`. Notably **not** `/` — that single exclusion is what
   keeps the matcher off the inside of URLs, and it is why `abc#1` and `v2.0#3`
   are not references.
2. **Terminator after.** The digit run must not be followed by an alphanumeric
   or `_`, which rules out `#12abc` and `#L12` line anchors.
3. **Code is skipped.** Fenced blocks (``` or `~~~`) and inline code spans are
   masked out before scanning. Literal examples live there — and so do hex
   colours, where `#123456` is indistinguishable from an issue number by shape
   alone. An unterminated fence masks to the end of the text, which is the safe
   direction to err in.

No cap is placed on the number of digits. It would kill 6-digit hex colours
outright, but at the cost of correctness on any repo whose issue numbers exceed
99999 — and rule 3 already handles the realistic cases.

### The known cost

`github.com/o/r#129` — a repo URL with a numeric fragment — matches **nothing**,
because the owner is preceded by `/`. Rule 1 is what keeps the scanner out of
URLs generally, so this case is the price of that. It is pinned by
`parse_pr_links_skips_a_repo_url_with_a_numeric_fragment` so it stays a
deliberate trade rather than something rediscovered as a bug.

## The behaviour change to watch

`P` now fires on threads where it used to say "no PR links found", and the
picker is busier — `closes #45` is a candidate now. That is the ticket's intent,
but it is the first thing to revisit if the feature feels noisy in daily use.
The lever is `match_bare` in `src/provider/types.rs`; dropping it leaves the
qualified form working on its own.

## Where it lives

| Concern | File |
|---|---|
| Scanner, `PrRef`, `IssueRef`, `PrLookup` | `src/provider/types.rs` |
| `issue(number:)` query, `map_pr_lookup` | `src/github/client.rs` |
| Provider trait signature | `src/provider/mod.rs` |
| Current-repo resolution, `pr_issue_ref` | `src/tui/app/pr.rs` |
| Repo-pinned jump | `src/tui/app/rows.rs` |
| Issue-case rendering | `src/tui/ui/pr.rs` |
| `o`/Enter jump-or-open | `src/tui/event/keys/pr.rs` |
