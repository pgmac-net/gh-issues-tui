# Local timezone for dates and times (#211)

## Behaviour

Every date and time the TUI shows is in the **system timezone**, resolved by
`chrono::Local` (the `TZ` environment variable, then `/etc/localtime`).
There is no config option and no zone marker; `TZ=Australia/Brisbane
gh-issues` overrides the zone for one run.

| Where | Shown as |
|---|---|
| List row, updated date | `2026-10-29` |
| Detail header, created / updated / closed | `2026-10-29` |
| Comment header | `2026-10-29 22:34` |
| Rate-limit reset | `22:45 (in 12m)` (was `12:45 UTC`) |

Date filters (`created`/`updated`/`closed` after/before) compare **local
days**, and the filter calendar's "today" is the local date. An issue updated
at `2026-10-28T22:00Z` is the 29th in Brisbane: the list shows `2026-10-29`
and `after 2026-10-29` includes it.

## Source timezone

Nothing to resolve at display time: every provider normalises to UTC on
ingest. GitHub and Linear send `Z` timestamps into `DateTime<Utc>`, and Jira's
`%z` offset is converted in `jira::parse_jira_dt`.

## Implementation

`src/localtime.rs` is the single conversion point. The `*_in` helpers are
generic over `chrono::TimeZone`; the plain wrappers (`fmt_date`,
`fmt_datetime`, `fmt_time`, `today`) bind `Local`. Filters use
`localtime::local_date_in` through timezone-generic inner functions
(`on_or_after_in` / `on_or_before_in`) so the day-boundary tests pin a
`FixedOffset` instead of depending on the host.

## Test fixtures

The shared `issue()` fixture in `src/tui/app/tests.rs` used to be created at
`00:00Z`, which is the previous local day anywhere west of UTC, so
`date_filters_bound_created` failed under `TZ=Pacific/Pago_Pago`. It now builds
timestamps at **local noon**, so the local day is the same on every host. No
single UTC instant works for offsets from -12 to +14.

## Out of scope

Relative times ("3h ago"), a timezone config option, and offset or
abbreviation display (abbreviations would need `chrono-tz`).
