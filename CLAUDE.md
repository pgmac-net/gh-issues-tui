# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                    # debug build
cargo build --release          # release build
cargo test                     # run all tests
cargo test <module>::tests     # run tests for one module (e.g. tui::app::tests)
cargo clippy --all-targets -- -D warnings    # lint (must pass with zero warnings)
cargo fmt                      # format in place
cargo fmt --check              # format check (used in CI)
```

No system dependencies beyond a Rust toolchain — TLS is rustls, no clipboard/keyring.

## Architecture

Three top-level modules wired together in `src/main.rs`:

| Module | Purpose |
|--------|---------|
| `config` | TOML config (`~/.config/gh-issues/config.toml`: `default_org`, `default_collapsed`). |
| `cwd_repo` | Detects the cwd's `origin` GitHub remote (`(owner, repo)`), best-effort via `git remote get-url origin`. |
| `github` | Async GitHub GraphQL v4 client + token resolution. |
| `tui` | Terminal UI (ratatui + crossterm). Owns the event loop. |

Startup org resolution in `main.rs`: `--org` flag → cwd git remote (owner, plus the repo name as the initial repo filter) → `default_org`. The detected repo filter is applied with `--org` only when the remote owner matches the flag.

### github/

- `auth.rs` — `resolve_token`: `--token` flag → `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token`. Injectable closures make the chain unit-testable.
- `client.rs` — `Client` (cheaply cloneable, one `reqwest::Client`). `org_issues` walks `repositoryOwner.repositories` (works for both organisations and user accounts) with cursor pagination and follows nested per-repo issue pagination — deliberately NOT the search API, which caps at 1000 results. Mutations: `add_comment`, `set_state`, `update_title`, `set_assignees` (resolves logins → node ids), `set_labels` (resolves names → label ids via `repo_labels`).
- `types.rs` — domain types (`Issue`, `RepoIssues`, `Comment`, `IssueState`). GraphQL response DTOs live privately in `client.rs`.

### tui/

- `app.rs` — All state and pure logic: `Filters` (text/repo/assignee/author/date bounds), `SortKey`, collapsible `Row` model (`RepoHeader`/`Issue`), `InputState` (char-indexed, UTF-8 safe), `Mode` (Normal/Input/FilterMenu/ConfirmState/Help). `rebuild_rows()` recomputes the visible list from data + filters + sort + collapsed set. This module has no I/O — it holds the bulk of the unit tests.
- `event.rs` — Async event loop: `tokio::select!` over crossterm `EventStream` and an mpsc channel of `AppEvent`s from spawned background tasks. All GitHub calls happen in spawned tasks; mutations send `MutationDone` which triggers a full refetch (simple consistency over optimistic updates).
- `ui.rs` — Pure render from `&App`. No state mutation in draw code.

## Key design invariants

- **Tokens never in config.** `Config` has no token field; resolution is env/CLI/`gh` only.
- **Pagination over search.** Issue fetch must stay on `repositoryOwner.repositories` → `issues` cursors. Do not switch to the GraphQL/REST search API — it silently caps at 1000 results org-wide.
- **Repo filter is exact-when-exact.** When the filter text exactly equals a loaded repo name (case-insensitive) only that repo matches; otherwise substring. Computed per `rebuild_rows` pass.
- **Org switch resets view state.** `App::switch_org` clears data, filters, collapse and seen-repo sets (keeps `include_closed`); callers must spawn a refetch.
- **`rebuild_rows` after any change** to filters, sort, collapse state, or data. Selection is clamped there; stale indices panic otherwise.
- **Collapse state keyed by repo name** (not index) so it survives reloads. `default_collapsed` (config default: true) is applied in `set_data` only to repos not yet in `seen_repos`, so manual expand/collapse choices always win over the config default. Exception: when the current filters leave exactly one repo group visible (`single_visible_repo`), that group defaults to expanded.
- **Panic hook** in `main.rs` restores the terminal before printing panics. Anything that touches terminal state must stay safe to drop in this path.
- **Closed issues are lazily fetched.** Startup fetches open-only unless `--all`; the first switch of the state filter away from `open` sets `include_closed` and refetches once.

## Release process

- Stable release: push a tag `v<major>.<minor>.<patch>` — `.github/workflows/release.yml` builds 4 platform binaries and creates a GitHub release.
- Pre-release: tag `v<major>.<minor>.<patch>-rcN`.
- CI on PRs: clippy (`-D warnings`), tests, release build on Linux/macOS/Windows (Windows `allow_failure`).
