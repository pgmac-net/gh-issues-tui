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

# Development log — picker type-ahead (2026-07-13)

Work driven by [pgmac-net/gh-issues-tui#9](https://github.com/pgmac-net/gh-issues-tui/issues/9), delivered in PR #17 on branch `9-picker-typeahead`.

## Process

Scope confirmed with Paul before planning: direct typing (not a `/`-prefix mode) and all pickers, not just the repo one. Plan approved on the ticket, then implemented in one PR.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Typing model | chars filter immediately; ↑/↓ navigate | Ticket's literal ask ("just start typing"); costs j/k/g/G/q inside pickers only |
| Index model | `select_idx` positional in the filtered view, commits map back via `picker_selected_original()` | Form pickers store indices into `FormOptions` lists and multi-select `[x]` marks key off original indices — value-based commits would silently break them |
| Shared handler | one `picker_common_key` + one `start_picker` entry point | Three picker modes (filter editor, form single, form multi) must not drift |
| No-match Enter | no-op (empty picker still closes) | Mis-typed filter shouldn't dismiss the picker and lose context |
| Filter row prefix | ASCII `/`, not 🔎 | Wide-emoji cell widths are unreliable across terminals; also crashed the pyte test driver (IndexError in wcwidth handling) — found live during verification |

## Diversions from plan

- Plan said "🔎 row"; shipped `/` row for the terminal-width reason above. Behaviour unchanged.

## Verification

- 100 unit tests (9 new), clippy `-D warnings`, `fmt --check` — green.
- Live pty+pyte drive over the 19-repo pgmac-net list: `F` → repo picker → typed `gh-i` → list narrowed to `gh-issues-tui` under the `/ gh-i█` row → Enter applied the repo filter → issue list collapsed to that repo.

# Development log — hide-empty-repos filter (2026-07-13)

Work driven by [pgmac-net/gh-issues-tui#20](https://github.com/pgmac-net/gh-issues-tui/issues/20), delivered in PR #21 on branch `20-hide-empty-repos-filter`. Direction chosen: Paul's ticket comment — a filter with a config default — over the original show-always / creation-picker / bare-toggle options.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Approach | filter-editor toggle + `hide_empty_repos` config default | Discoverable in the existing `F` editor; default-true keeps the clean view; config-default-on-clear matches `default_collapsed` behaviour |
| "Empty" semantics | zero **visible** issues | One rule for never-had-issues repos and filter-emptied groups; rides the existing `visible.is_empty()` line in `rebuild_rows` |
| Fetch | always include empty repos; exclude archived (`isArchived: false`) and issues-disabled (`hasIssuesEnabled`) repos | Instant client-side toggle, no refetch; archived/disabled repos can never be useful here. Forks kept — they can carry issues |
| Field UX | Enter toggles yes/no in place (`FILTER_HIDE_EMPTY_IDX` intercept) | Boolean row; a picker would be two keystrokes for two values |
| Reset + indicator | `clear_filters`/`switch_org` restore the config default; `filters_active()` counts only deviation | Paul's explicit spec: clearing filters returns to the config setting, and a config default isn't an "active" filter |
| `Filters::default()` | manual impl with `hide_empty: true` | A derived `false` default would have leaked "show empties" into every `Filters::default()` call site and silently changed filtered-to-zero behaviour |

## Diversions from plan

None — implemented as approved.

## Verification

- 107 unit tests (7 new), clippy `-D warnings`, `fmt --check` — green.
- Live pty+pyte drive against pgmac-net: 19 repos at baseline → filter toggled to `no` → 46 repos with `(0)` headers → `F`→`c` → 19 again; repo-filtered to the empty `ansible-role-apotd`, `n` opened the create form targeting it (first-issue creation, the limitation deferred from #10, now closed out).

# Development log — colour code by priority (2026-07-14)

Work driven by [pgmac-net/gh-issues-tui#26](https://github.com/pgmac-net/gh-issues-tui/issues/26), delivered in PR #27 on branch `26-priority-colour`.

## Process

1. **Plan** posted to the ticket and approved before implementation: colour issue titles with their `priority:*` label's own GitHub colour rather than introducing a config-driven priority→colour map.
2. **Implementation** — `Issue::priority_label()` in `github::types` (first label starting `priority:`, case-insensitive, matching the existing filter/form convention), and a `title_style()` helper in `tui::ui` used by both the list rows (`issue_item`) and the detail pane header.
3. **Delivery** — PR #27; tests and clippy green.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Colour source | The priority label's GitHub colour | Already the user's source of truth; works with any priority naming (`high`, `P1`, …); zero new config |
| What gets coloured | Title only (list + detail header) | Labels/dates keep their own colours so rows stay scannable |
| Multiple priority labels | First wins | Degenerate case; consistent with `priority_options()` ordering |
| Unparsable label colour | `label_fallback` theme colour | Reuses the existing `label_color` parser and its fallback path |
| Rejected alternative | `priority_colors` map in colour profiles | Priority values are free-form; can be layered on later if wanted |

## Diversions from plan

None — implemented as approved.

## Verification

- `cargo test` — 112 passed (5 new for `priority_label`).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.

# Development log — sort by priority (2026-07-14)

Work driven by [pgmac-net/gh-issues-tui#28](https://github.com/pgmac-net/gh-issues-tui/issues/28), delivered in PR #29 on branch `28-priority-sort`.

## Process

1. **Plan** drafted in plan mode with three clarifications resolved up front (direction toggle behaviour, unknown-value ranking, tie-breaking), approved, and posted to the ticket.
2. **Implementation** — `Issue::priority_rank()` beside the existing `priority_label()`; `SortKey::Priority` variant slotted into the cycle before the wrap; comparator in `sort_issues` with a direction-independent tie-break.
3. **Delivery** — PR referencing the ticket; also carries the `title_style` unit tests written while diagnosing the "invisible priority colours" report (root cause was ededed label colours from the Linear migration, fixed by recolouring the labels, no code change).

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Rank order | none/unknown 0 < low 1 < medium 2 < high 3 < urgent 4 | Matches org's four priority values; descending puts urgent first, unprioritised last |
| Unknown values (`priority:P1`) | rank 0, same as no priority | Org only uses the four values; anything else is noise |
| Direction | global `S` toggle, no special-casing | Consistent with every other sort key |
| Tie-break | `updated_at` descending in both directions | Applied after the direction reverse so ties are always most-recent-first |
| `next()` cycle position | after `author`, before wrapping to `updated` | Keeps existing muscle memory intact |

## Diversions from plan

None.

## Verification

- `cargo test` — 122 passed (10 new: 4 rank mapping/edge cases in `types.rs`, 4 sort/tie/cycle in `app.rs`, plus 2 `title_style` tests from the colour diagnosis).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
