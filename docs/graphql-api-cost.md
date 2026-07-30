# GraphQL API cost (#107)

A full org fetch cost **202 points** and now costs **4**. This documents why, so
the change is not undone by a well-meaning "just add the labels back inline".

## The cost law

GitHub's GraphQL API bills against a 5,000 **points**/hour budget — not 5,000
requests. Points are not proportional to the data returned; they are driven by
the connections a query nests.

Measured against `pgmac-net` (57 repos, 122 open issues) by adding
`rateLimit { cost nodeCount }` to each variant:

| `reposFirst` | `issuesFirst` | nested connections under `issues` | nodeCount | cost |
|---|---|---|---|---|
| 50 | 100 | `assignees` + `labels` | 155,050 | **101** |
| 50 | 100 | `labels` only | 105,050 | 51 |
| 50 | 100 | `assignees` only | 55,050 | 51 |
| 50 | 100 | `labels(first: 1)` + `assignees(first: 1)` | 15,050 | **101** |
| 50 | 40 | `assignees` + `labels` | 32,050 | 41 |
| 50 | 10 | `assignees` + `labels` | 15,550 | 11 |
| 100 | 40 | `assignees` + `labels` | 124,100 | 81 |
| 100 | 100 | *none* | 10,100 | **1** |
| 100 | 1 | *none* | 200 | 1 |

Every sample fits:

```
cost = 1 + n_nested_connections × (reposFirst × issuesFirst) / 100
```

Three consequences, none of them obvious:

1. **Nested page size is irrelevant.** `labels(first: 1)` costs exactly the same
   as `labels(first: 20)` — 101 either way. Shrinking nested pages does nothing.
2. **`reposFirst` is free** once no connection is nested inside `issues`. The
   largest page is therefore also the cheapest, because it needs fewer requests
   for the same points. Hence `REPOS_PAGE = 100`.
3. **`nodeCount` is not cost.** The 15,050-node variant and the 155,050-node
   variant both cost 101.

## What it cost before

`issue_fields!` carried `assignees(first: 10)` and `labels(first: 20)` inline,
with `reposFirst: 50`:

- 101 points per repo page × 2 pages = **202 points per refresh**
- at the default `refresh_interval = 300`, 12 refreshes/hour = **2,424
  points/hour idle**
- every mutation triggers a further full refetch (`docs/architecture.md`,
  "Consistency")

Which is how a 5,000/hour budget disappears in an afternoon. The reported
symptom — "~200+ API calls when updating" — was this number: points, not calls.

## The current design

**Phase 1 — bulk list, no nested connections** (`ORG_ISSUES_QUERY`,
`REPO_ISSUES_QUERY`). Scalars only, `reposFirst: 100`, `issuesFirst: 100`.
Whole org, one request, **cost 1**.

**Phase 2 — hydration** (`ISSUE_HYDRATE_QUERY`). `nodes(ids: [...])` is a plain
list, not a connection, so the two connections inside it are charged once for
the batch instead of once per issue slot. Batches of 100 (GitHub's id limit),
**cost 2 per batch**.

Measured end-to-end against `pgmac-net`:

```
phase1 page 1: cost=1
repos=57 issues=122
phase2 batch 1 (100 ids): cost=2
phase2 batch 2 (22 ids):  cost=1
TOTAL = 4     (was 202)
```

Hydration is **atomic**: `org_issues` awaits both phases and returns fully
populated issues. `provider::priority` derives priority *from* labels and the
label filters read them, so rows must never be built from empty label sets —
a progressive render would sort the list, then reshuffle it when hydration
landed.

## The guard

`issue_fields_has_no_nested_connections` fails the build if any connection
reappears in the bulk query — by name for the likely candidates, and by
rejecting `first:` outright for anything else. This is the one test to read
before "simplifying" the two-phase fetch back into one: the regression it
catches has no visible symptom until the hourly budget is gone.

`bulk_queries_report_their_cost` pins `rateLimit { cost }` onto all three
queries. The client accumulates it across a fetch
(`RateLimitStore::begin_fetch` / `add_cost` / `end_fetch`) and the list footer
shows `API <remaining>/<limit> (last fetch <n>)`, so the burn is visible while
using the tool rather than discovered at exhaustion.

## Second amplifier: comment fetches

Independently of points, `nav` spawned a comments request on *every* selection
change while the detail pane was open — holding `j` meant one request per row,
with no cache.

`App::load_comments` now resolves two cases without any request:

- the thread was already fetched this cycle (`comment_cache`)
- `comment_count == 0`, which the bulk fetch already told us

Both settle `detail.comments` to a loaded thread rather than leaving it `None`,
so the pane never waits for a response that will never come. The cache is
cleared wholesale by `set_data` and `switch_org` (a refetch can reveal comments
added elsewhere) and a single entry is invalidated after a mutation.

## Rejected

- **Search API** (`org:X is:issue`) — 1 or 2 points for the whole org, but it
  silently caps at 1,000 results. Still rejected; see `docs/architecture.md`.
- **Shrinking page sizes** — the obvious fix, and worth only ~2.5x (202 → 81)
  because it attacks the wrong term. Superseded by removing the nested
  connections entirely.
- **Lazy hydration of visible rows only** — cheapest of all, but label filters
  and priority sort would operate on partially loaded data.
