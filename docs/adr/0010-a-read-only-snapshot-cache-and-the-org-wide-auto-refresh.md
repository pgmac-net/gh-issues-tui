# 10. A read-only snapshot cache, and auto-refresh covers the org

## Status

Accepted (#226). Supersedes the auto-refresh part of ADR 0008 rule 1.

## Context

The first load of an org takes a few seconds, and every start pays it. #226 asked for a caching layer between the app and the backends, which the app talks to and which replays changes to the backend. A write-back cache like that needs a queue, retries and conflict resolution. But writes are not what is slow. Since ADR 0005 every write already goes straight to the backend and patches the list with the issue the write returned, and `keep_newer_loaded` stops an older read from undoing it.

## Decision

1. **The cache is read-only.** Writes go to the backend first, as before, and patch both the list and the snapshot. Nothing is queued or replayed, so the only conflict is a cached copy against a fresh copy of the same issue. `keep_newer_loaded` (newer `updated_at` wins) already decides that.
2. **A snapshot is drawn at once, and the backend always gets the last word.** At startup a usable snapshot is shown immediately. A refresh of the filtered scope then always starts, with no age threshold, and settles over it. The info bar shows the snapshot's age until that refresh lands.
3. **A snapshot is usable only when its scope covers the wanted scope** (`FetchScope::covers`). Otherwise startup is a normal blocking load. A snapshot never stands in for data it does not hold.
4. **Auto-refresh covers the org**, at the loaded scope, not the filtered scope. `Ctrl+R` stays the filtered scope. Otherwise a day spent on one repo leaves the rest of the snapshot hours old at the next start. An org fetch is about 4 GraphQL points (#107), so a 5-minute timer costs about 48 points an hour.
5. **Jev answers are keyed by what was sent.** A readiness or search answer is cached under a hash of the exact state sent, plus model and prompt version. It is stale exactly when its input changes, so no `updated_at` and no TTL. A label change that is not sent to Jev does not cost a re-evaluation.

## Considered options

- **Write-back with replay** (the ticket's first shape): offline edits, but a queue, retry policy and a merge UI to solve a problem we don't have. Rejected; it can return as its own ticket.
- **Skip the startup refresh when the snapshot is young**: saves one fetch, but lets the snapshot decide what is true. Rejected.

## Consequences

- Issue bodies of private repos now sit on disk. Files are `0600` in a `0700` directory, written atomically, and documented, with `cache = false` and `--no-cache` to opt out.
- Move must return the moved issue so it can be patched in place (no org refetch). Create still refetches the org.
- A filter change never shrinks the loaded scope. Widening checks the snapshot before fetching, and its fetch is a *load* of missing data, not a refresh.
