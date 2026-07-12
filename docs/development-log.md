# Development log — initial implementation (2026-07-05)

Work driven by [pgmac-net/homelabia#128](https://github.com/pgmac-net/homelabia/issues/128), phases tracked in sub-issues #129 (repo), #130 (implementation), #131 (docs).

## Process

1. **Repo creation via IaC** — `gh-issues-tui` was added to `config/repos.yaml` in [terraform-github](https://github.com/pgmac-net/terraform-github) (PR #14) under `repos.public`, mirroring the `docker-registry-walk` entry. The PR plan showed exactly 4 resources (repository, branch protection, topics, vulnerability alerts); apply ran automatically on merge to main.
2. **Scaffold** — the empty repo was bootstrapped with a single direct commit to main (README/LICENSE/.gitignore only, unavoidable before a base branch exists); all implementation went through PR #1.
3. **Implementation** — single PR with the full feature set, CI, release workflow, tests and docs.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Language/framework | Rust + ratatui | Matches docker-registry-walk; proven CI + release patterns in the org |
| Fetch strategy | `organization.repositories` → `issues` cursor pagination | GitHub search API caps at 1000 results org-wide |
| Auth | flag → `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token` | Zero-config on machines with `gh`; no stored secrets |
| Mutations | whole-set replacement via `updateIssue` for assignees/labels | One mutation instead of add/remove pairs; input pre-filled with current set |
| Consistency after mutation | full refetch | Simpler than optimistic patching; org fetch is fast enough |
| Closed issues | lazy one-time refetch on first filter switch | Keeps startup fast for the common open-issues case |
| TLS | rustls | No OpenSSL/system deps; leaner CI than docker-registry-walk |

## Diversions from plan

- **README apply instructions were stale**: terraform-github's README describes a manual `apply.yml` workflow, but the live `terraform.yml` applies automatically on push to main. No manual apply step was needed.
- **No `in-progress` label existed** in homelabia; created `status:in-progress` to match the existing `status:*` family rather than inventing a new naming scheme.
- **Initial commit to main**: the never-commit-to-main rule can't apply to an empty repository; one minimal scaffold commit bootstrapped the default branch, everything else via PR.
- **`reqwest` feature rename**: the planned `rustls-tls` feature no longer exists in reqwest 0.13; used `rustls` + `rustls-native-certs` + `http2`.

## Verification

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, 29 unit tests — green locally and in CI (Linux, macOS, Windows).
- Live smoke test against `pgmac-net`: loaded 106 issues across 18 repos; repo grouping rendered with counts.
- Scripted keystroke session (pseudo-tty): searched for a scratch issue, added a comment, closed it via `x`+`y` — both verified with `gh issue view` afterwards; scratch issue deleted.

# Development log — auto-refresh (2026-07-12)

Work driven by [pgmac-net/gh-issues-tui#8](https://github.com/pgmac-net/gh-issues-tui/issues/8), delivered in PR #11 on branch `8-auto-refresh`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code. Confirmed interpretation with Paul: keep manual `r` reload, add an automatic background refresh, and verify the manual path genuinely refetches.
2. **Manual reload verification** — traced `r` → `spawn_fetch` → `Client::org_issues`: every press is a fresh GraphQL POST with full repo/issue cursor pagination; reqwest does not cache POSTs. Already correct — no fix needed.
3. **Implementation** — config key, CLI flag, event-loop ticker, gating predicate, selection preservation, tests, docs — single PR.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Default interval | 300 s, `0` disables | Frequent enough to feel live; well inside GraphQL rate budget for ~20-repo orgs |
| Configuration | `refresh_interval` config key + `--refresh` flag (flag wins) | Matches the existing `default_collapsed`/`--all` split of persistent vs per-run settings |
| Ticker mechanics | `tokio::time::interval_at` (first tick one period out), `MissedTickBehavior::Delay` | `interval()` fires immediately, which would double-fetch at startup; Delay avoids burst catch-up after long stalls |
| Tick gating | `App::should_auto_refresh`: not loading, no rate-limit lockout, mode is Normal/Help | Never stacks fetches, respects the existing rate-limit lockout, never refreshes under an input box, menu, or confirmation |
| Selection across refetch | preserve by issue id in `set_data`, fall back to clamped index | Selection was index-based; a background refresh inserting/removing rows would silently move the highlight mid-navigation. Benefits manual reload too |
| Status wording | `auto-refreshed …` vs `loaded …` via an `auto_refreshing` flag on `App` | User can tell an unattended refresh happened without a separate notification channel |

## Diversions from plan

None — implemented as approved.

## Verification

- 81 unit tests (6 new: config default/explicit/zero parsing, selection preserved and clamped across `set_data`, gating predicate), clippy `-D warnings`, `fmt --check` — all green.
- Live smoke test: `--org pgmac-net --refresh 4` in a sized pseudo-tty (`script` + `stty`; note a bare `script` pty has zero size and ratatui renders nothing) — observed `loaded 107 issues across 19 repos` then `auto-refreshed 107 issues across 19 repos` after the ticker fired.

# Development log — right arrow into detail pane (2026-07-13)

Work driven by [pgmac-net/gh-issues-tui#12](https://github.com/pgmac-net/gh-issues-tui/issues/12), delivered in PR #14 on branch `12-right-arrow-detail`.

## Process

1. Plan posted to the ticket and approved before code. Key observation enabling a clean design: `→` on an issue row was already a no-op (a visible issue row implies its repo group is expanded), so the key was free to take on "move into the detail pane" without losing anything.
2. Implementation in one small PR: `App::enter_detail`, the `→` handler split in `event.rs`, help overlay + README key table, tests.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| `→` on issue row, pane closed | open the pane focused, same as `Enter` (comments load) | `→` consistently means "go deeper"; ticket asked for intuitive symmetry with `←` |
| `→` on issue row, pane open | flip focus only, no comment refetch | Mirror of `←` backing out; refetch would be wasted API budget |
| `→` on repo header | unchanged (expand group) | Existing muscle memory; headers have no detail view |
| Logic placement | `App::enter_detail` returning `Option<issue id>` for the needed comment fetch | Keeps `event.rs` thin and the behaviour unit-testable without I/O |
| Help overlay | split the single `← / →` row into two rows | Combined description of both meanings exceeded the 52-column help box |

## Diversions from plan

None — implemented as approved.

## Verification

- 84 unit tests (3 new for `enter_detail`: header no-op, closed-pane open+fetch, open-pane focus-only), clippy `-D warnings`, `fmt --check` — green.
- Live pty+pyte drive (`.claude/skills/verify` recipe) against `pgmac-net`: `]`, `j`, `→` opened the detail pane showing the selected issue's body and comment thread; `←`/`→` flipped focus with the pane staying open; `q` closed back to the full-width list.

# Development log — new-issue form (2026-07-13)

Work driven by [pgmac-net/gh-issues-tui#10](https://github.com/pgmac-net/gh-issues-tui/issues/10), delivered in PR #16 on Paul's `pgmac/create-new-issue` branch (his first-pass commit 62bbe29 preserved underneath).

## Process

1. Reviewed Paul's first pass (single-line title prompt → `createIssue`): kept the `n` trigger and client structure; superseded the interim `InputKind::CreateIssue` flow; `createIssue` now returns `issue { number url }`; the per-create repo-id lookup was replaced by the id riding along with the form-options fetch.
2. Plan + review posted to the ticket; scope decisions confirmed before implementation (all 8 fields, multi-line body, continue Paul's branch, zero-issue repos deferred).
3. Form built by mirroring the filter-editor machinery (field list → per-field popup/input) rather than inventing a new pattern.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Form machinery | mirror `FilterMenu`/`SelectField` | Proven in-repo pattern; users already know the interaction |
| Body editor | line-wise composition of the existing `InputState` | `tui-textarea` 0.7 (latest) pins ratatui 0.29, incompatible with our 0.30 — duplicate-crate type clash. `InputState` already solves UTF-8 char-boundary editing per line |
| Options fetch | one query (repo id, labels, assignable users, open milestones, Projects V2); issue types separate + failure-tolerated | issue types are an org feature; an unavailable field must not kill the whole form |
| Priority | single-select over `priority:*` labels, merged (deduped) into `labelIds` | GitHub has no native priority; matches the org convention the filter code already uses |
| Project | `addProjectV2ItemById` after creation | `CreateIssueInput` has no ProjectsV2 field |
| Multi-select | Space toggles a working set on `App`, Enter commits, Esc discards | Keeps `IssueForm` state clean and popup cancellation cheap |
| Stale options | dropped by repo name | Same idiom as `AppEvent::Comments` |

## Diversions from plan

- `tui-textarea` dropped for the version conflict above (noted on the ticket when found). Everything else as approved.

## Verification

- 92 unit tests (11 new), clippy `-D warnings`, `fmt --check` — green.
- Live pty+pyte E2E against the real API from inside the repo clone (auto-scoped): `n` → typed title, two-line body, toggled `documentation` in the labels multi-select, submitted from `[ Create issue ]` — issue #15 appeared in the refetched list; `gh issue view` confirmed body `"Line one\nline two"` and the label; scratch closed and deleted.
