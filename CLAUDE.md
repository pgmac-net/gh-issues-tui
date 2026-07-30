# CLAUDE.md

Rust/ratatui org-wide GitHub/Linear/Jira issues TUI. Module map, provider mapping tables, data-fetch strategy, mutations, and security decisions: `docs/architecture.md` (note: its module tree predates the `tui/app/`, `tui/event/`, `tui/ui/` directory split below — trust `ls src/tui/` over that one diagram). Deep-dive docs for specific features (clickable URLs, detail-pane scrolling, inline new-issue form, PR summary popup) are also under `docs/`.

## Commands

```sh
cargo build --release
cargo test <module>::tests     # e.g. tui::app::tests
cargo clippy --all-targets -- -D warnings    # must pass with zero warnings
cargo fmt --check               # used in CI
```

No system dependencies beyond a Rust toolchain — TLS is rustls, no keyring. Clipboard copy (`y`) uses the OSC 52 terminal escape sequence, not a system clipboard library, so it works over SSH.

## Startup resolution (`main.rs`)

- Org: `--org` flag → cwd git remote (owner, plus repo name as initial filter) → `default_org`. The detected repo filter only applies alongside `--org` when the remote owner matches the flag.
- Provider: `--provider` flag → `provider` config key → `"github"`. Unknown names error with the supported list (`github`, `linear`, `jira`).

## tui/ — state machine invariants

Three layers, each a directory (`app/` state and pure logic, `event/` key handling and spawned work, `ui/` rendering) — nothing exceeds ~400 production lines, a feature is found by filename, and each layer has a private `prelude` module for its shared imports.

- `app/` has no I/O and computes no screen geometry; `App`'s inherent methods split across several `impl App` blocks, one per concern file. Tests mostly drive a whole `App` through shared fixtures (`app/tests.rs`) rather than testing one file in isolation.
- **State that resets together is grouped together**: `App` holds `detail: DetailState`, `pr: PrState`, `picker: PickerState`, `editor: EditorState` (plus `filters: Filters`, `issue_form: Option<IssueForm>`) — each with a `Default` and named reset methods, so a reset's intent is recorded once. `focus` stays flat despite reading as detail-pane state, because `switch_org`/`cycle_focus` set it too.
- **Partial resets are named, and their differences are deliberate.** `PrState::close()` keeps `links` (reopening the picker shouldn't refetch); `PrState::default()` discards everything; `PrState::refresh()` keeps `target`. Do not "tidy" these into one `default()` — `app/pr.rs` tests pin each one.
- **Chrome fields are deliberately not grouped** (`loading`, `auto_refreshing`, `status`, `rate_limit`, `rate_limit_error`) — ~119 reference sites, nothing resets them as a set, wrapping them would be pure churn.
- `layout.rs` computes all screen geometry once, as pure functions over `Rect`; both `ui::draw` and `event.rs`'s scroll clamps call these so the renderer and key handler can't drift. `from_terminal_size()` is the only place the real terminal is read.
- **Wrapping is owned by `tui::linkmap`, not ratatui** — detail regions pre-wrap lines with `linkmap::wrap` and render with `Paragraph` wrapping off. `linkmap::wrapped_height` counts rows the same way so measured heights (`body_content_height`/`comment_height`/`comment_offset`) always match the drawn layout.
- **Clickable URLs (#80)** are terminal-native OSC 8 hyperlinks, not app-handled clicks — no mouse capture. `ui::apply_hyperlinks` performs the OSC 8 buffer surgery after `Paragraph` draws, pinned with `CellDiffOption::ForcedWidth` so the escape bytes don't disturb ratatui's layout/diff. Full detail: `docs/clickable-urls.md`.
- **Inline editing** reuses one `Mode::CommentEditor` widget for three targets (`NewComment`, `EditComment`, `EditBody`) via `EditorTarget` on `App`. An empty submission is discarded except for `EditBody` (clearing a description is valid).
- New-issue form (`n`) is one inline form (`Mode::IssueForm`), not per-field modals — text fields edit in place, choice fields (assignees/labels/type/priority/project/milestone) are the one deliberate exception, using picker popups since those lists benefit from type-ahead. Full detail: `docs/inline-new-issue-form.md`.
- Pickers have type-ahead: `select_idx` is positional within the **filtered** view, so every commit path maps back via `picker_selected_original()`.
- `P` (detail pane) summarises a linked PR — full detail: `docs/pr-summary-modal-actionable.md`, and `docs/pr-summary-markdown.md` for the shared markdown renderer (#102). `App::pr_target` guards against a stale response landing after the popup retargeted or closed. The popup's body links are OSC 8 only, never `Tab` targets — `PrRow` holds one URL, a body line can hold several.

## Key design invariants

- **Tokens never in config.** `Config` has no token field; resolution is env/CLI/`gh` only.
- **Pagination over search.** Issue fetch must stay on `repositoryOwner.repositories` → `issues` cursors — the search API silently caps at 1000 results org-wide.
- **No nested connections in the bulk issue query.** GraphQL cost is `1 + n_nested × (reposFirst × issuesFirst)/100` — a connection inside `issues` is billed per issue slot requested, whatever its own page size. `issue_fields!` is scalars only; labels and assignees are hydrated by `Client::hydrate_issues` via `nodes(ids:)` batches. Inlining them again costs 100x with no visible symptom until the hourly budget is gone (`issue_fields_has_no_nested_connections` guards it). See `docs/graphql-api-cost.md`.
- **Repo filter is exact-when-exact.** Filter text exactly matching a loaded repo name (case-insensitive) matches only that repo; otherwise substring. Computed per `rebuild_rows` pass.
- **Org switch resets view state.** `App::switch_org` clears data, filters, collapse and seen-repo sets (keeps `include_closed`); callers must spawn a refetch.
- **`rebuild_rows` after any change** to filters, sort, collapse state, or data — stale selection indices panic otherwise.
- **Selection survives refetches by issue id.** `set_data` re-locates the previously selected issue after rebuilding rows; a vanished issue falls back to the clamped index.
- **Collapse state keyed by repo name** (not index) so it survives reloads. `default_collapsed` applies only to repos not yet in `seen_repos`, so manual choices always win. Exception: when filters leave exactly one repo group visible, that group defaults expanded.
- **Panic hook** in `main.rs` restores the terminal before printing panics. Anything touching terminal state must stay safe to drop in this path.
- **Closed issues are lazily fetched.** Startup fetches open-only unless `--all`; the first state-filter switch away from `open` sets `include_closed` and refetches once.
- **Empty repos are fetched, visibility is a filter.** `org_issues` keeps zero-issue repos (excludes archived/issues-disabled at the query) so `hide empty repos` toggles instantly client-side.

## Release process

Push a tag `v<major>.<minor>.<patch>` (or `-rcN` for pre-release) — `.github/workflows/release.yml` builds 4 platform binaries. CI on PRs: clippy (`-D warnings`), tests, release build on Linux/macOS/Windows (Windows `allow_failure`).
