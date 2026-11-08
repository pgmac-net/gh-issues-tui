# Caching (#226)

The first load of an org took about 7.5 seconds on `pgmac-net` (202 issues across 62 repos), and every start paid it. Now the last loaded issues are kept on disk and drawn at once, and the backend is asked in the background. Decision record: [ADR 0010](adr/0010-a-read-only-snapshot-cache-and-the-org-wide-auto-refresh.md). Terms: [CONTEXT.md → Caching](../CONTEXT.md#caching-226).

## What changed for the user

| | Before | Now |
|---|---|---|
| Start, cache warm | list after ~7.6 s | list after ~0.3 s, `cached 2h ago · refreshing…` until the refresh lands |
| Start, refresh fails | error, empty list | cached list stays usable, `cached 2h ago · refresh failed` |
| Auto-refresh | the filtered scope | the whole org, so the next start is fresh even after a day on one repo |
| `Ctrl+R` | the filtered scope at the filters' scope | the filtered scope, at a scope that also covers what is loaded |
| Move (`m`) | the org was refetched | the moved issue is placed, nothing refetched |
| Widening a filter | `filters widened, refreshing…`, a fetch | served from the snapshot when it covers the new filters, else `filters widened, loading…` |
| Readiness badge / semantic search | asked again every session | an unchanged issue is not sent again |

Measured live against `pgmac-net` with an isolated cache: cold 7.65 s, warm 0.26 s, `--no-cache` 7.79 s.

## The cache is read-only

Writes go to the backend first, exactly as before (ADR 0005), and the list and snapshot follow from the issue the write returned. Nothing is queued or replayed, so the only conflict is a cached copy against a fresh copy of the same issue. `keep_newer_loaded` already settles that: the newer `updated_at` wins. A write-back cache with offline edits was considered and rejected (see the ADR).

## Files

```text
~/.cache/gh-issues/                      ($XDG_CACHE_HOME honoured)
    label-ranks.json                     inferred label ranks (#156), unchanged, not touched by --no-cache
    <provider>@<host>/<org>/
        snapshot.json                    the last loaded issues and the scope they were fetched at
        readiness.json                   readiness answers, one per issue
        run_jobs.json                    jobs of finished workflow runs, by run and attempt (#266)
        search.json                      search answers for the last 20 queries
```

macOS: `~/Library/Caches/gh-issues/`. Windows: `%LOCALAPPDATA%\gh-issues\`. The host tells two Jira sites apart (`acme.atlassian.net`); GitHub is `github.com`, Linear `linear.app`.

Every file is written atomically (temp file, then rename) and privately: `0600` files in `0700` directories on Unix, since the snapshot holds the titles and bodies of private issues. A missing, corrupt or older-version file reads as empty, never as an error.

## The snapshot

- **Drawn at startup when usable**: its scope covers what the resumed filters want (`Loaded::covers`). Anything else is treated as missing and the start is a normal load. A snapshot of open issues is never drawn under an `all` filter.
- **Always followed by a fetch.** No age threshold: the snapshot only saves the wait, the backend decides what is true.
- **Saved after every settle**: an org or repo load, an issue refresh or patch, a removal, a move. Each bumps `App.data_gen`; `CacheIo` writes 2 seconds after the first change it sees, and on quit.
- **`w` (switch org)** opens the new org's files. The old org's last debounce window (at most 2 s of settles) is not written; its previous snapshot stands.
- **Age**: `App.fetched_at` is when the data was last fetched (an org or filtered fetch landing), saved as the snapshot's `saved_at`. A one-issue or one-repo refresh does not make the whole list newer.

## Scopes never shrink on a refresh

Every filtered or org fetch asks for `App::refresh_scope()`: the filters' scope widened by `FetchScope::union` to cover what is loaded. So narrowing a filter and pressing `Ctrl+R` keeps the closed issues you loaded, and toggling `open` / `all` / `open` fetches at most once. The union keeps a login only when both sides name the same one, merges label sets (labels are any-of), and takes the earlier `since`.

## Widening a filter

`CacheIo::widen_from_snapshot` runs before `refetch_if_widened`. When the snapshot on disk covers the new filters it is folded in: membership is the union of the list and the snapshot, `keep_newer_loaded` keeps the newer copy of each issue, and the list is then labelled with the snapshot's age (`cached 2h ago · Ctrl+R refreshes`). Otherwise the fetch goes out as before, with the status `filters widened, loading…`: it loads data the list does not hold, it does not refresh what it holds.

In practice the snapshot rarely covers more than the list, because refreshes never shrink the loaded scope. It helps after a start narrowed to one repo, when the org-wide snapshot from an earlier session is still on disk.

## Jev answers

Readiness and search answers are kept under the hash of the **whole request** (state, model and question text), FNV-1a 64-bit (`cache::stable_hash`), which unlike `DefaultHasher` is the same in every build.

- **Readiness**: one entry per issue id. `begin_readiness` builds the state as before, hashes the request, and uses a kept answer instead of asking. The in-memory judgements are still dropped on every refresh; the disk lookup is what makes asking again free. A label change is not sent to Jev, so it costs nothing.
- **Search**: per normalised query (case and spacing folded), the last 20. `begin_semantic_search` sends only the candidates without a kept answer, and the kept hits are merged with the new ones when they land. If every candidate is kept, the hits apply at once and nothing is sent.
- Both are used only with the `send_issue_text` consent and `cache = true`. Without the consent the files are neither read nor written.

## Turning it off

- `--no-cache`: wipe this org's directory (snapshot, readiness, search) and start as on a first run. The cache is rebuilt as you go.
- `cache = false` in `config.toml`: never read or write any of the three.
- `Ctrl+R` is still the way to refresh now.

## Code

| What | Where |
|---|---|
| Files, paths, hash, wipe | `src/cache/{mod,fsio,snapshot,answers}.rs` |
| Snapshot ↔ list, age indicator, widen fold | `src/tui/app/cache.rs` |
| When to read and write | `src/tui/event/cache_io.rs` (`CacheIo`) |
| Refresh scope union | `FetchScope::union`, `App::refresh_scope` |
| Org auto-refresh | `spawn_org_refresh` |
| Move placed in place | `App::apply_move`, `AppEvent::IssueMoved`, `with_issue_move` |
| Cache host per backend | `IssueProvider::host` |
