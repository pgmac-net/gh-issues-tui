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

No system dependencies beyond a Rust toolchain — TLS is rustls, no keyring. Clipboard copy (`y`) uses the OSC 52 terminal escape sequence, not a system clipboard library, to keep this true and to work over SSH.

## Architecture

Three top-level modules wired together in `src/main.rs`:

| Module | Purpose |
|--------|---------|
| `config` | TOML config (`~/.config/gh-issues/config.toml`: `default_org`, `provider`, `default_collapsed`, `refresh_interval`, `hide_empty_repos`, `copy_format`, `color_profile` + `[color_profiles.*]`). |
| `cwd_repo` | Detects the cwd's `origin` GitHub remote (`(owner, repo)`), best-effort via `git remote get-url origin`. |
| `provider` | Backend-neutral layer: `IssueProvider` trait, domain types, `ProviderError`, name → provider factory. |
| `github` | Async GitHub GraphQL v4 client + token resolution; implements `IssueProvider`. |
| `linear` | Async Linear GraphQL client + key resolution; implements `IssueProvider`. Teams = repo groups; native priority ↔ synthetic `priority:*` labels. |
| `tui` | Terminal UI (ratatui + crossterm). Owns the event loop; talks only to `Provider` (`Arc<dyn IssueProvider>`). |

Startup org resolution in `main.rs`: `--org` flag → cwd git remote (owner, plus the repo name as the initial repo filter) → `default_org`. The detected repo filter is applied with `--org` only when the remote owner matches the flag.

Startup provider resolution in `main.rs`: `--provider` flag → `provider` config key → `"github"`. `provider::build` maps the name to a boxed provider and resolves its credentials; unknown names error with the supported list (`provider::SUPPORTED` = `github`, `linear`).

### linear/

- `auth.rs` — `resolve_key`: `--token` → `LINEAR_API_KEY` → `LINEAR_TOKEN`. No local-CLI fallback (no `gh` equivalent). Personal key sent raw in `Authorization` (no `Bearer`).
- `mod.rs` — priority int (`1=urgent … 4=low`, `0=none`) ↔ `priority:*` label-value mapping, and synthetic-label helpers. Synthetic label ids carry the `linear-priority:` prefix and never reach Linear.
- `client.rs` — Linear GraphQL client, `impl IssueProvider`. **Teams = repo groups** (`repo` = team key; `org` arg ignored — workspace is the key's). Native priority is folded into a synthetic `priority:*` label on read (`to_issue`) so the app's sort/colour/filter/picker need no Linear special-casing; `set_labels` peels a `priority:*` **name** to the native field, `create_issue` peels a synthetic priority **id** from `label_ids`, both resolving real labels against `real_repo_labels` (which excludes the synthetics). `set_state` resolves the issue's team workflow states and moves it to the lowest-position state of the wanted category. Single-assignee (0-or-1 vec). `supports_pr_summary = false`; milestones/issue-types empty; comment count not fetched in the bulk list.

### provider/

- `mod.rs` — `IssueProvider` trait (`async_trait`, so it's usable as `Arc<dyn IssueProvider>` — the `Provider` alias the event loop clones into spawned tasks). Core methods are required; capability methods default to `Err(ProviderError::Unsupported)` paired with a `supports_*` probe (today: `pull_request` / `supports_pr_summary`) — the UI checks the probe and shows a status message instead of calling a doomed fetch. New backends (Linear #24, Jira #25) = new trait impl + factory arm in `build`.
- `types.rs` — backend-neutral domain types (`Issue`, `RepoIssues`, `Comment`, `IssueState`, `PrRef`, `PrSummary`, `FormOptions`, `NewIssueParams`, `RateLimitData`). `Issue.id` is an opaque string (GitHub: GraphQL node id). `parse_pr_links` scans text for explicit `github.com/{owner}/{repo}/pull/{N}` links — deliberately not bare `#N` shorthand, which is ambiguous between an issue and a PR in an issues tool. Types stay here even when only one backend can fetch them (`PrSummary`): the data is neutral, the fetch is a capability.
- `error.rs` — `ProviderError` (`Http`/`Api`/`Shape`/`RateLimited`/`ResourceLimited`/`Unsupported`) + `RATE_LIMIT_MSG_PREFIX` (the event loop classifies stringified task errors by this prefix).

### github/

- `auth.rs` — `resolve_token`: `--token` flag → `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token`. Injectable closures make the chain unit-testable.
- `client.rs` — `Client` (cheaply cloneable, one `reqwest::Client`). `org_issues` walks `repositoryOwner.repositories` (works for both organisations and user accounts) with cursor pagination and follows nested per-repo issue pagination — deliberately NOT the search API, which caps at 1000 results. Mutations: `add_comment`, `set_state`, `update_title`, `set_assignees` (resolves logins → node ids), `set_labels` (resolves names → label ids via `repo_labels`). `pull_request` fetches a `PrSummary` in one query: title/state/diffstat, review status, checks on the head commit, the PR's own Actions runs, and recent runs on the repo's default branch (the `CheckRun`/`StatusContext` GraphQL union is flattened into one `ContextNode` DTO keyed off `__typename`).
- The `impl IssueProvider for Client` block at the bottom of `client.rs` is thin delegation — inherent methods win name resolution inside the impl, so each trait method calls the real implementation. GitHub opts into the PR-summary capability (`supports_pr_summary` → `true`).
- **Adaptive page-size backoff.** A query whose combined repo/issue page size trips GitHub's GraphQL complexity budget returns `ProviderError::ResourceLimited` instead of a raw error dump (detected by GraphQL error `type` or a "Resource limits" message). `Client::graphql_with_backoff` retries the same cursor with halved `PageSizes` (`REPOS_PAGE`/`ISSUES_PAGE` down to `MIN_REPOS_PAGE`/`MIN_ISSUES_PAGE`) — cursors are opaque positions, so shrinking `first` mid-fetch is valid and the full dataset still arrives, just across more requests. `org_issues` shares one `PageSizes` across its whole run (never grows back) so a fetch that trips the limit once starts small for the rest of the org. Other GraphQL errors now join each entry's `message` field instead of dumping the errors array as JSON.
- GraphQL response DTOs live privately in `client.rs`; the domain types they map into live in `provider/types.rs`.

### tui/

- `theme.rs` — `Theme` (resolved UI colours, `Default` = original scheme) + `ColorProfile` (per-field optional overrides deserialized from `[color_profiles.<name>]`; colours parse via ratatui's `Color` serde: names/hex/index). `Config::resolve_theme` picks the profile named by `color_profile`; a missing name is a startup error. `ui::draw` takes `&Theme` — no colour constants in `ui.rs`.
- `app.rs` — All state and pure logic: `Filters` (text/repo/assignee/author/date bounds), `SortKey`, collapsible `Row` model (`RepoHeader`/`Issue`), `InputState` (char-indexed, UTF-8 safe), `Mode` (Normal/Input/FilterMenu/ConfirmState/Help). `rebuild_rows()` recomputes the visible list from data + filters + sort + collapsed set. This module has no I/O — it holds the bulk of the unit tests.
- Detail view is a 40/60 split pane (`detail_open` on `App`; `Focus` = which pane has keys). List navigation live-follows into the pane via `nav()` in `event.rs` (fetches comments only when the selected issue id actually changed); `AppEvent::Comments` drops stale responses by issue id. Esc/q close the split from either pane; Tab/BackTab cycle focus.
- `event.rs` — Async event loop: `tokio::select!` over crossterm `EventStream`, an mpsc channel of `AppEvent`s from spawned background tasks, and an auto-refresh ticker (`refresh_interval` config / `--refresh` flag, 0 disables; gated per tick by `App::should_auto_refresh` — skips while loading, rate-limited, or in any interactive mode other than Normal/Help). All GitHub calls happen in spawned tasks; mutations send `MutationDone` which triggers a full refetch (simple consistency over optimistic updates). When the detail pane is open on an issue, `MutationDone` also refetches that issue's comment thread (`comments_refresh_target`) — otherwise a just-added comment stays invisible until the selection changes.
- New-issue form (`n`): `IssueForm` on `App` mirrors the filter-editor pattern — `Mode::IssueForm` field list, `Mode::IssueFormSelect`/`IssueFormMulti` pickers (multi = Space toggles into `App::multi_selected`, committed on Enter), `Mode::IssueFormBody` multi-line editor (`BodyEditor` = one UTF-8-safe `InputState` per line). Picker options come from `Client::repo_form_options` per repo (`AppEvent::FormOptions`, stale-dropped by repo name); issue types are queried separately and failure-tolerated. `build_params` merges the priority pick (a `priority:*` label) into `label_ids`; a chosen ProjectV2 is applied post-create via `addProjectV2ItemById`.
- Pickers have type-ahead: `App::select_filter` narrows the view (`filtered_select()` yields `(original index, text)` pairs); `select_idx` is positional within the **filtered** view, so every commit path maps back via `picker_selected_original()` — the form pickers and multi-select `[x]` marks store original-option indices. `start_picker` is the single entry point (resets the filter); `picker_common_key` in `event.rs` is the shared key handler (chars filter — so j/k/q are filter text inside pickers; ↑/↓ navigate, Backspace/Ctrl+U edit).
- Editing an existing issue reuses the same picker: `p` (set priority, `Mode::PrioritySet`, single-select) and `l` (edit labels, `Mode::LabelsSet`, multi-select) both fetch `repo_labels` on demand and guard staleness with a `*_pick_issue: Option<String>` id captured when the fetch starts — the options response is dropped if the mode, selection, or target issue moved on before it landed.
- `P` (detail pane only) summarises a linked PR: `App::collect_pr_links` scans the selected issue's body + `detail_comments` (`parse_pr_links`), body first then comments in display order. Zero links sets a status message; one link fetches directly; several open `Mode::PrPicker` (reuses the `select_options`/`picker_common_key` picker machinery) before fetching. `Mode::PrSummary` shows the result; `App::pr_target` guards `AppEvent::PrSummary` against a stale response landing after the popup retargeted or closed (mirrors the `*_pick_issue` pattern above). `AppEvent::PrSummary` boxes its `Result<PrSummary, String>` payload (clippy `large_enum_variant`).
- `ui.rs` — Pure render from `&App`. No state mutation in draw code.

## Key design invariants

- **Tokens never in config.** `Config` has no token field; resolution is env/CLI/`gh` only.
- **Pagination over search.** Issue fetch must stay on `repositoryOwner.repositories` → `issues` cursors. Do not switch to the GraphQL/REST search API — it silently caps at 1000 results org-wide.
- **Repo filter is exact-when-exact.** When the filter text exactly equals a loaded repo name (case-insensitive) only that repo matches; otherwise substring. Computed per `rebuild_rows` pass.
- **Org switch resets view state.** `App::switch_org` clears data, filters, collapse and seen-repo sets (keeps `include_closed`); callers must spawn a refetch.
- **`rebuild_rows` after any change** to filters, sort, collapse state, or data. Selection is clamped there; stale indices panic otherwise.
- **Selection survives refetches by issue id.** `set_data` re-locates the previously selected issue after rebuilding rows (background auto-refresh must not move the highlight); a vanished issue falls back to the clamped index.
- **Collapse state keyed by repo name** (not index) so it survives reloads. `default_collapsed` (config default: true) is applied in `set_data` only to repos not yet in `seen_repos`, so manual expand/collapse choices always win over the config default. Exception: when the current filters leave exactly one repo group visible (`single_visible_repo`), that group defaults to expanded.
- **Panic hook** in `main.rs` restores the terminal before printing panics. Anything that touches terminal state must stay safe to drop in this path.
- **Closed issues are lazily fetched.** Startup fetches open-only unless `--all`; the first switch of the state filter away from `open` sets `include_closed` and refetches once.
- **Empty repos are fetched, visibility is a filter.** `org_issues` keeps zero-issue repos (excludes archived and issues-disabled ones at the query) so the `hide empty repos` filter toggles instantly client-side. "Empty" = zero *visible* issues — one rule for never-had-issues repos and filtered-to-zero groups alike (`rebuild_rows`). The toggle's reset value is the config default (`hide_empty_default` on `App`): `clear_filters` and `switch_org` restore it, and `filters_active()` counts it only when it deviates. The filter-editor row flips in place on Enter (`FILTER_HIDE_EMPTY_IDX` intercept in `handle_filter_menu_key`).

## Release process

- Stable release: push a tag `v<major>.<minor>.<patch>` — `.github/workflows/release.yml` builds 4 platform binaries and creates a GitHub release.
- Pre-release: tag `v<major>.<minor>.<patch>-rcN`.
- CI on PRs: clippy (`-D warnings`), tests, release build on Linux/macOS/Windows (Windows `allow_failure`).
