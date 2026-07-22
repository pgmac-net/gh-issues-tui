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

# Development log — set priority via picker (2026-07-14)

Work driven by [pgmac-net/gh-issues-tui#30](https://github.com/pgmac-net/gh-issues-tui/issues/30), delivered in PR #31 on branch `30-priority-picker`.

## Process

1. Requested by Paul mid-session while reviewing the priority sort work: `p` on the selected issue opens a picker of the repo's `priority:*` labels.
2. **Implementation** — pure helpers in `app.rs` (`priority_set_options`, `priority_label_set`), a `Mode::PrioritySet` picker reusing the generic type-ahead machinery, `AppEvent::PriorityOptions` fetched via the existing `Client::repo_labels`, and the existing `set_labels` mutation via `with_issue`.
3. **Verification** — unit tests plus a live tmux-driven session: no-priority-labels repo path, picker ordering, current-priority pre-highlight, Esc cancel.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Options source | fetch `repo_labels` on `p` | Setting requires the label to exist on the repo; loaded issue data only shows labels in use |
| Option order | `—` (clear), low → urgent, unknown values last alphabetically | Matches Paul's stated ranking; clear is always first like other pickers |
| Initial highlight | the issue's current priority | One `Enter` re-confirms; adjacent keys move one step |
| Mutation | whole-set replace via existing `set_labels` | Battle-tested path (`l` key); new code only computes the name set (pure, unit-tested) |
| Staleness | `priority_pick_issue` id guard on response arrival and on Enter | Selection can drift while options load; refetch can remove the issue |
| Repo without `priority:*` labels | status message, no picker | Nothing pickable; popup would only offer `—` |

## Diversions from plan

- Live mutation was not exercised end-to-end: creating a scratch issue for the test was declined by the session's permission gate, and mutating a real issue's priority was not acceptable. The mutation path itself (`set_labels`) predates this change and is covered by existing usage; the new label-set computation is unit-tested.

## Verification

- `cargo test` — 116 passed (4 new for options/label-set helpers).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- tmux-driven live session: `p` on a repo without priority labels → status message; on homelabia issue #114 → picker with `— clear —, low, medium, high, urgent`, current `priority:high` pre-highlighted, Esc cancels cleanly. (Also visually confirmed the #26 title colouring with the recoloured labels.)

# Development log — multiline text input overhaul (2026-07-14)

Work driven by [pgmac-net/gh-issues-tui#22](https://github.com/pgmac-net/gh-issues-tui/issues/22), delivered in PR #32 on branch `22-multiline-input`.

## Process

1. **Plan** agreed with four up-front clarifications (visual-row Up/Down, readline Ctrl+U, whitespace word boundaries, apply everywhere), posted to the ticket.
2. **Implementation** — readline-style ops on `InputState` (word motion, word delete, kill to start/end, home/end, delete-under-cursor), `BodyEditor` delegation plus a pure word-wrap layout (`wrap_lines`/`cursor_row`/`VisualRow`) with visual-row Up/Down, shared popup-width helper, and a shared block-cursor renderer replacing the inserted `█` in both the bottom input line and the body popup.
3. **Verification** — 22 new unit tests plus a live tmux session driving the release build.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Wrap type | soft/visual only — buffer never rewritten | Hard-wrapping would mangle the submitted markdown |
| Wrap algorithm | break after last space in window; hard-break over-long words | Simple, predictable, testable; space stays on the first row |
| Wrap-boundary cursor | belongs to the next visual row (except at line end) | Deterministic mapping; matches editor conventions |
| Up/Down | visual rows via `wrap_lines` recomputed per keypress | Body text is small; no cache invalidation complexity |
| Width source | `body_popup_width(frame width)` shared by ui + events | Renderer and key handler must agree on geometry |
| Cursor rendering | `Modifier::REVERSED` on the char under the cursor | Ticket complaint: inserted `█` shifts text and hides the char; also fixes the bottom-line cursor being stuck at the end |
| Word ops in body | line-local | Matches existing left/right; crossing lines wasn't asked for |
| Ctrl+U | readline delete-to-start (was clear-all) | Per clarification; cursor-at-end still clears everything |

## Diversions from plan

None. (Also fixed in passing: the README sort-key list was missing `priority` from #28.)

## Verification

- `cargo test` — 140 passed (22 new).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- tmux live session: paragraph wraps at popup width across 3 rows; Ctrl+← hops words; Ctrl+W deleted "long " mid-paragraph with correct re-flow; Ctrl+A/E/K behave on the logical line; block cursor confirmed as `ESC[7m` reversed video sitting ON the character; single-line comment input shows the cursor mid-string after two Lefts (previously always drawn at the end).

# Development log — comment/input popups (2026-07-16)

Work driven by [pgmac-net/gh-issues-tui#36](https://github.com/pgmac-net/gh-issues-tui/issues/36), delivered in PR #38 on branch `36-comment-multiline-input`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Sonnet 5).
2. **Code inspection** — traced the existing `Mode::Input(InputKind)` single-line path (`app.rs`/`event.rs`/`ui.rs`) and the `Mode::IssueFormBody` multi-line `BodyEditor` used for the new-issue description, to reuse the latter's word-wrap/cursor/visual-row logic for comments rather than building a second editor.
3. **Implementation** — new `Mode::CommentEditor` + `App::comment_editor: BodyEditor`; extracted the readline/visual-row key handling shared by the comment and description editors into `apply_body_editor_key` in `event.rs`; moved every `Mode::Input(kind)` render from the bottom status line into a centered popup with a new stateless horizontal-scroll helper (`input_scroll_skip`).

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Comment submit key | `Ctrl+S` | `Ctrl+Enter` isn't reliably distinguishable from plain `Enter` across terminals; `Enter` stays "insert newline" for consistency with the description editor |
| Comment editor widget | Reuse `BodyEditor` + `apply_body_editor_key` | Avoids a second multi-line implementation; new-issue description and comments now share one code path |
| Single-line input rendering | Centered popup (same visual language as the multi-line popups) instead of the bottom status line | Bottom line is one row — too little room to show cursor position clearly on longer values; consistent look across all text entry |
| Long single-line values | Stateless horizontal scroll (`input_scroll_skip(cursor, width)`, recomputed each frame) | No extra scroll-offset state to keep in sync with cursor moves; window always derives directly from `(cursor, width)` |
| `InputKind::Comment` | Removed | Comments no longer go through the generic single-line `Input` mode at all |

## Diversions from plan

None — implemented as approved.

## Verification

- `cargo test` (148 passing), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — all clean.
- Live pty-driven session against the real `pgmac-net` org: opened the comment popup on a real issue, typed two lines via `Enter`, discarded with `Esc` (status: "comment discarded", no mutation sent); opened the title popup with a value longer than the box and confirmed the horizontal scroll kept the cursor visible, then discarded with `Esc`.

# Development log — edit-labels picker for existing issues (2026-07-16)

Work driven by [pgmac-net/gh-issues-tui#37](https://github.com/pgmac-net/gh-issues-tui/issues/37), delivered in PR #40 on branch `37-label-picker-existing-issue`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Opus after the user declined the Sonnet switch prompt; noted in the ticket).
2. **Code inspection** — traced the old `l` key path (`Mode::Input(InputKind::Labels)`, free-text comma-separated, `split_csv`) and the existing multi-select picker infra already shared by the new-issue form's labels field and the filter editor's priority/status pickers (`start_picker`/`select_options`/`multi_selected`/`picker_common_key`/`picker_items`).
3. **Implementation** — modelled directly on the existing single-select `p` (set priority) flow for an existing issue: new `Mode::LabelsSet`, `App::label_pick_issue: Option<String>` staleness guard, `AppEvent::LabelOptions`, `spawn_label_options`, `handle_labels_set_key`. Removed `InputKind::Labels` and its `submit_input` arm entirely.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| New mode | `Mode::LabelsSet`, mirrors `Mode::PrioritySet` | Same generic picker widget, just `multi=true`; keeps the two existing-issue pickers structurally parallel |
| Options source | fetch `repo_labels` on `l` | Same reasoning as the priority picker — a label must exist on the repo to be settable |
| Pre-check | issue's current labels matched case-insensitively into `multi_selected` before `start_picker` | Same pattern the new-issue form already uses when reopening a multi-select field |
| Mutation | whole-set replace via existing `set_labels` | Same call the old free-text flow used; only the source of the name list changed |
| `InputKind::Labels` | removed entirely | Dead once `l` no longer opens a text input; no other caller |
| No labels on repo | status message, no picker | Nothing pickable — matches the priority picker's empty-repo behaviour |

## Diversions from plan

- Implemented on Opus instead of the Sonnet tier the plan recorded — user declined the mid-task model switch prompt.
- Manual smoke test (press `l` on a real issue, confirm pre-checked/toggle/commit against `gh issue view`) was left undone in the PR checklist — no interactive terminal session against a live repo was exercised this session. Covered instead by unit tests on the new picker mechanics.

## Verification

- `cargo test` — 154 passed (6 new).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- `README.md`, `docs/architecture.md`, and `CLAUDE.md` updated to describe the picker behaviour in place of the old free-text description.

# Development log — refresh comment thread after adding a comment (2026-07-16)

Work driven by [pgmac-net/gh-issues-tui#39](https://github.com/pgmac-net/gh-issues-tui/issues/39), delivered in PR #42 on branch `39-refresh-comments-after-add`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Sonnet 5).
2. **Code inspection** — traced the mutation-consistency model: `MutationDone` triggers a full org refetch (issue metadata, comment counts) unconditionally, but the detail pane's rendered comment thread (`detail_comments`) was only refetched by `nav()` on selection change — a just-added comment stayed invisible until the user navigated away and back.
3. **Implementation** — pure helper `comments_refresh_target(&App) -> Option<String>` (pane open + issue selected → its id), called from the `MutationDone` handler alongside the existing `spawn_fetch`, inside the same rate-limit gate.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Scope | all mutations, not just comment-adds | Matches the repo's "full refetch, simple consistency" philosophy; one extra call per user mutation, only while the pane is open |
| Loading state | no `detail_comments = None` reset before refetch | Avoids flashing "loading comments…"; pane keeps showing the current thread until the fresh one lands |
| Staleness | reused the existing `Comments` handler's selection-id guard | Already covers ordering races; no new guard needed |
| Scroll position | left untouched, no auto-scroll-to-bottom | Wrapped-line count only exists in the renderer; computing "bottom" would duplicate render logic — out of scope |
| Auto-refresh ticker | not touched | Same staleness exists there (external comments landing while the pane is open) but outside the ticket's scope; flagged as a possible follow-up |

## Diversions from plan

None — implemented as approved.

## Verification

- `cargo test` — 157 passed (3 new).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- Live smoke test: tmux-driven session against the release build and the real `pgmac-net` org. Created scratch issue #41, opened its detail pane ("no comments"), typed a comment via `c`, submitted with `Ctrl+S` — the comment appeared in the pane immediately, visible even while the list refetch was still showing "loading…" in the header, confirming the comments refetch fired independently of the list refetch. Scratch issue deleted afterward.

# Development log — copy short URL to clipboard (2026-07-20)

Work driven by [pgmac-net/gh-issues-tui#46](https://github.com/pgmac-net/gh-issues-tui/issues/46), on branch `46-copy-short-url`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Sonnet 5).
2. **Code inspection** — traced how a selected issue's owner/repo/number are already available (`App::org`, `RepoIssues::repo`, `Issue::number`), and how the existing `status: Option<String>` field drives the footer toast used by every other mutating key (`o`, `c`, `x`, ...).
3. **Clipboard mechanism reconsidered mid-implementation** — the plan (posted before reading `CLAUDE.md`) picked `arboard` for system-clipboard access. Once implementation started, the repo's own architecture doc surfaced: "No system dependencies beyond a Rust toolchain — TLS is rustls, no clipboard/keyring." `arboard` also silently fails over headless SSH, the primary way this TUI gets used against the homelab's repos. Flagged to the user immediately; switched to an OSC 52 terminal escape sequence instead — zero new system deps, and it works over SSH (with tmux passthrough handled explicitly, since tmux does not forward OSC 52 by default).
4. **Implementation** — `App::selected_short_ref()` renders `copy_format` against the selected issue; `y` in normal mode calls it and writes the OSC 52 sequence straight to stdout, interleaved safely with ratatui's rendering since terminals consume the escape without displaying it.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Clipboard mechanism | OSC 52 escape sequence, not a clipboard crate | Repo's stated "no clipboard/keyring" invariant + must work over SSH (see Process #3) |
| Key | `y` | Free in normal mode; conventional "yank" pairing with the existing `o` (open in browser) |
| Reference format | `{owner}/{repo}#{number}`, default `pgmac-net/gh-issues-tui#46` | Matches the ticket's example format (with the real org substituted for the ticket's shorthand `pgmac/`); pastes directly into `gh` and Claude Code |
| Configurability | new `copy_format` string in `config.toml`, `{owner}`/`{repo}`/`{number}` placeholders | Matches the ticket's explicit ask; simple string substitution, no new parser |
| tmux | explicit `\ePtmux;...\e\\` passthrough wrap when `$TMUX` is set | tmux does not forward raw OSC 52 to the outer terminal without the wrapper; this TUI is routinely run inside tmux |

## Diversions from plan

- Clipboard mechanism changed from `arboard` (as planned and posted to the ticket) to OSC 52, discovered to conflict with `CLAUDE.md`'s stated architecture invariant partway through implementation. Flagged to the user, who chose OSC 52. `CLAUDE.md` and `docs/architecture.md` updated to describe the OSC 52 approach.

## Verification

- `cargo test` — 162 passed (6 new: 2 config, 3 `selected_short_ref`, existing suite untouched otherwise).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- Manual smoke test of `y` against a live terminal session left for the PR review step.

# Development log — inline comment editor (2026-07-22)

Work driven by [pgmac-net/gh-issues-tui#51](https://github.com/pgmac-net/gh-issues-tui/issues/51), delivered in PR #53 on branch `51-inline-comment-editor`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Sonnet 5).
2. **Clarifying questions** — two open questions resolved with the user before planning: what `c` should do when the detail pane is closed (auto-open it, so no behaviour is lost from the list view), and what the save/cancel UI should look like (rendered `[ Save ]  [ Cancel ]` buttons with `Tab` focus-cycling, rather than a border-title hint).
3. **Implementation** — replaced the centered `Mode::CommentEditor` popup (`draw_comment_editor_popup`) with an inline section carved out of the bottom third of the detail pane. Added `CommentFocus` (`Editor`/`Save`/`Cancel`) alongside the existing `Focus` enum, `App::start_comment_editor` (mirrors `enter_detail`'s auto-open-and-return-fetch-id pattern), and `comment_pane_width` (mirrors `body_popup_width`'s render/key-handler shared-formula pattern, approximating the detail pane's 60%-of-frame width since it sits behind a `Layout::horizontal` split rather than a fixed popup width).

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| `c` with pane closed | auto-open the pane (fetch comments) and start the editor in one step | User's explicit choice — no behaviour lost versus the old popup, which worked from anywhere |
| Save/cancel UI | rendered `[ Save ]  [ Cancel ]` button row, `Tab`/`Shift+Tab` cycles editor→Save→Cancel | User's explicit choice over a border-title-only hint; `Ctrl+S`/`Esc` kept as shortcuts from any focus so muscle memory from the old popup still works |
| Section height | `Constraint::Percentage(33)` of the detail pane, `Constraint::Min(1)` for the thread above it | Matches the ticket's "about 33% of the total height" ask directly |
| Width formula | new `comment_pane_width`, same clamp-and-subtract-borders shape as `body_popup_width` | One source of truth shared between the renderer and the key handler's visual-row up/down math, same reasoning as the existing popup helpers — exact for a fixed-width popup, an approximation here since the real width comes from a `Layout` solver, judged good enough since it only affects cursor keep-visible scrolling |
| Already-open pane | `start_comment_editor` doesn't reset `detail_comments` when the pane was already open | Avoids re-fetching or blanking an already-loaded thread just because the editor opened; only closed-pane opens need `spawn_comments` |

## Diversions from plan

None — implemented as approved.

## Verification

- `cargo test` — 185 passed (9 new: 4 `start_comment_editor` in `app.rs`, 5 focus-cycling/discard/save in `event.rs`).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- Live smoke test: tmux-driven session against the release build and the real `pgmac-net` org, on issue #51 itself. Pressed `c` from the list view (pane closed) — pane auto-opened, comment thread loaded, inline section appeared at the bottom with the editor focused; typed multi-line text and confirmed wrap; `Tab` moved focus to `[ Save ]` (confirmed reversed-video highlight via raw ANSI capture), `Enter` submitted — verified posted via `gh issue view --json comments`, then deleted the scratch comment. Repeated with `Tab`×2 to `[ Cancel ]`, `Enter` discarded — confirmed via `gh api .../comments` that no second comment was created.

# Development log — readable GraphQL resource-limit errors + page-size backoff (2026-07-22)

Work driven by [pgmac-net/gh-issues-tui#53](https://github.com/pgmac-net/gh-issues-tui/issues/53), delivered in PR #54 on branch `53-graphql-resource-limit-error`.

## Process

1. **Plan approval** — implementation plan posted to the ticket and approved before any code, rated STANDARD (implemented on Sonnet 5).
2. **Code inspection** — traced the `f` key handler (`event.rs`) that upgrades `include_closed` and refetches on the first switch away from the open-only state filter, into `Client::org_issues`/`graphql` (`client.rs`) where the raw GraphQL `errors` array was being stringified straight into the status line, and the `GithubError` enum (`error.rs`) that had no variant for a resource-limit response.
3. **Implementation** — added `GithubError::ResourceLimited`, classified via a new `errors_contain_resource_limited` (matches known `type` values or a `Resource limits` message substring, since GitHub's exact error `type` for this case isn't consistently documented and the ticket's own paste of the error was truncated before the `type` value). Added `PageSizes` (repos/issues page sizes with a `shrink()` halving method) and `Client::graphql_with_backoff`, which retries a query from the same cursor with smaller pages on that error. Wired both the top-level `ORG_ISSUES_QUERY` loop and the nested per-repo `REPO_ISSUES_QUERY` pagination in `org_issues` through one shared `PageSizes` for the whole fetch.

## Decisions

| Decision | Choice | Why |
|----------|--------|-----|
| Detection | match GraphQL error `type` OR a `Resource limits` message substring | GitHub's `type` value for this error isn't documented and the ticket's pasted error was cut off before it; the message text is the one thing confirmed from the actual failure |
| Backoff scope | one `PageSizes`, shared and never grown back across a whole `org_issues` run | A query that overflows the complexity budget once is likely to again for the same org; growing back would just re-trigger the same failure on the next repo |
| Backoff shape | halve both repos and issues together down to independent floors (5 / 10) | The error path in the ticket's example pointed deep into nested fields (`repositories.nodes[45].issues.nodes[7].comments.totalCount`), i.e. combined complexity — not clearly attributable to one dimension, so both shrink together |
| Other GraphQL errors | join each entry's `message` field instead of `errors.to_string()` | Same status-line readability problem existed for any GraphQL error, not just resource-limit ones; minimal fix, same pattern as the existing rate-limit path |

## Diversions from plan

None — implemented as approved.

## Verification

- `cargo build --release`, `cargo test` — 190 passed (7 new: resource-limit detection by type and by message-only, message-joining incl. raw-JSON fallback, and the page-shrink sequence down to its floor).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- No live smoke test: reproducing GitHub's actual resource-limit response requires an org large enough to trip the complexity budget on a closed-issue fetch, which isn't available in this environment. Verified logically instead — cursor pagination is unaffected by page size (GraphQL cursors are opaque), and the new unit tests exercise the exact error-classification and shrink-sequence logic the live path depends on.
