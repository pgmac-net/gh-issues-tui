# gh-issues — architecture and design notes

Built for [pgmac-net/homelabia#128](https://github.com/pgmac-net/homelabia/issues/128).

## Overview

`gh-issues` is a single-binary Rust TUI (ratatui + crossterm + tokio) that browses and manages GitHub issues across a whole organisation. The module layout deliberately mirrors [docker-registry-walk](https://github.com/pgmac-net/docker-registry-walk), the org's existing Rust TUI.

```
src/
├── main.rs          CLI (clap), panic hook, wiring
├── config.rs        ~/.config/gh-issues/config.toml (default_org, provider, copy_format, ...)
├── provider/
│   ├── mod.rs       IssueProvider trait, Provider alias (Arc<dyn>), name → provider factory
│   ├── types.rs     backend-neutral domain types: Issue / RepoIssues / Comment / IssueState / ...
│   └── error.rs     ProviderError (thiserror), incl. Unsupported for capability gaps
├── github/
│   ├── auth.rs      token chain: --token → GITHUB_TOKEN → GH_TOKEN → gh auth token
│   └── client.rs    GraphQL v4 client: org-wide fetch + mutations; impl IssueProvider
├── linear/
│   ├── auth.rs      key chain: --token → LINEAR_API_KEY → LINEAR_TOKEN
│   ├── client.rs    Linear GraphQL client; impl IssueProvider
│   └── mod.rs       priority int ↔ priority:* label mapping, synthetic-label helpers
├── jira/
│   ├── auth.rs      env creds: JIRA_BASE_URL + JIRA_EMAIL + JIRA_API_TOKEN (Basic auth)
│   ├── client.rs    Jira Cloud REST client; impl IssueProvider
│   └── mod.rs       priority name ↔ priority:* mapping, ADF flatten/wrap, Jira datetime parse
└── tui/
    ├── app.rs       all state + pure logic (filters, sort, rows, input)
    ├── event.rs     async event loop, background tasks (talks to Provider, never a concrete client)
    └── ui.rs        pure render
```

## Provider abstraction ([#63](https://github.com/pgmac-net/gh-issues-tui/issues/63))

The TUI is written against `provider::IssueProvider`, a backend-neutral async trait covering the core issue operations (org-wide fetch, comments, create, mutations, label/form lookups, rate-limit state). The event loop holds a `Provider` (`Arc<dyn IssueProvider>`) and clones it into spawned tasks exactly as it used to clone the concrete client; `github::Client` implements the trait by thin delegation to its inherent methods.

One backend is chosen per session: `--provider` flag → `provider` config key → `"github"`. The factory (`provider::build`) resolves per-provider credentials — GitHub keeps its existing token chain — and rejects unknown names with the supported list. Linear ([#24](https://github.com/pgmac-net/gh-issues-tui/issues/24)) and Jira ([#25](https://github.com/pgmac-net/gh-issues-tui/issues/25)) slot in as new trait impls plus a factory arm.

Backend-specific features are **capabilities**: trait methods with safe defaults (`Err(ProviderError::Unsupported)`) paired with a `supports_*` probe the UI checks before offering the affordance. Today the only capability is the linked-PR summary popup (`supports_pr_summary` / `pull_request`) — GitHub opts in; a provider that doesn't gets a status-bar message instead of a doomed fetch. Domain types stay in `provider/types.rs` even when only one backend can fetch them today (e.g. `PrSummary`): the data is neutral, the fetch is a capability.

Deliberately out of scope for #63: mixed-source views (multiple providers in one list) and renaming the org/repo terminology — both revisit when a second provider lands.

### Linear provider ([#24](https://github.com/pgmac-net/gh-issues-tui/issues/24))

The second backend. Selected explicitly (`--provider linear` / `provider = "linear"`); a cwd git repo never implies Linear. Auth is a Linear personal API key (`--token` → `LINEAR_API_KEY` → `LINEAR_TOKEN`), sent raw in the `Authorization` header (no `Bearer` — that's the personal-key convention).

Concept mapping onto the shared domain types:

| Domain type | Linear source | Notes |
|---|---|---|
| `RepoIssues` | Team | `repo` = team key, `repo_url` = team URL. `org_issues` ignores its `org` arg (the workspace is the key's). |
| `Issue.number` | `issue.number` | Per-team. |
| `Issue.state` | `state.type` | `completed`/`canceled` → Closed, else Open. |
| `Issue.assignees` | `issue.assignee` | Single-assignee → a 0-or-1 vec. |
| `Issue.labels` | issue labels **+ synthetic `priority:*`** | Native priority (`1=urgent … 4=low`, `0=none`) is folded into a `priority:*` label so the app's existing sort/colour/filter/picker code needs no Linear special-casing. |
| `Issue.closed_at` | `completedAt ?? canceledAt` | |

**Priority round-trip.** Linear priority is a native field, but the UI works in `priority:*` labels. The provider bridges both directions:
- **Read:** `to_issue` inserts a synthetic `priority:<value>` label from the native int.
- **Picker/form:** `repo_labels` and `repo_form_options` append the four synthetic `priority:*` entries (ids prefixed `linear-priority:`) so the `p` picker and new-issue form have something to show. (Side effect: the `l` label editor also lists them — selecting one there sets priority, which is harmless.)
- **Write:** `set_labels` peels a `priority:*` *name* to the native `priority` field and resolves only the real names to team label ids; `create_issue` peels a synthetic priority *id* out of `label_ids` the same way. Real label ids are resolved against `real_repo_labels` (the synthetic entries are never sent to Linear).

**Capabilities.** `supports_pr_summary` stays `false` (no GitHub PR links in Linear), so the `P` keybind degrades to a status message via the #63 capability gate. Milestones and issue types have no Linear equivalent and stay empty; `projects` maps to Linear projects. Comment count is not fetched in the bulk list query (it appears when the detail pane loads the thread).

Close/reopen has no single flag in Linear — state is a per-team workflow object — so `set_state` first resolves the issue's team states and moves it to the lowest-position state of the wanted category (`completed` to close; a non-done state to reopen).

### Jira provider ([#25](https://github.com/pgmac-net/gh-issues-tui/issues/25))

The third backend and the only **REST** one (`/rest/api/3`, Jira Cloud). Selected explicitly (`--provider jira` / `provider = "jira"`). Credentials come from env — `provider::build` has no `Config`, and Jira needs more than a token: `JIRA_BASE_URL`, `JIRA_EMAIL`, and the token (`--token` → `JIRA_API_TOKEN`), combined into HTTP Basic auth (`base64(email:token)`).

Concept mapping onto the shared domain types:

| Domain type | Jira source | Notes |
|---|---|---|
| `RepoIssues` | Project | `repo` = project key, `repo_url` = `{site}/browse/{KEY}`. `org` arg ignored. |
| `Issue.id` | Issue **key** (`PROJ-123`) | Used directly in every mutation URL. |
| `Issue.number` | Numeric suffix of the key | `key_to_number`. |
| `Issue.body` | `description` (ADF) | Flattened to text via `adf_to_text`; write paths wrap text back with `text_to_adf`. |
| `Issue.state` | `status.statusCategory.key` | `done` → Closed, else Open. |
| `Issue.assignees` | `assignee` | Single → 0-or-1 vec. |
| `Issue.labels` | `labels[]` (strings) **+ synthetic `priority:*`** | Native priority (`Highest…Lowest`) folded into a `priority:*` label; Jira labels have no ids. |
| `Issue.closed_at` | `resolutiondate` | Parsed with `parse_jira_dt` (Jira's `+0000` offset isn't RFC 3339). |
| `comment_count` | `fields.comment.total` | Available in the list fetch (unlike Linear). |

**Fetch** is two-phase (like Linear): page `/project/search`, then page each project's issues via `GET /search` with JQL `project = "KEY" ORDER BY updated DESC` (`AND statusCategory != Done` unless closed are included), `startAt`/`total` pagination.

**Priority round-trip** mirrors Linear: read folds native→`priority:*`; `repo_labels`/`repo_form_options` append the four synthetic entries (ids prefixed `jira-priority:`); `set_labels` peels a `priority:*` *name* to the native `priority` field, `create_issue` peels a synthetic *id* from `label_ids`. Unlike Linear/GitHub, real labels need no id resolution — Jira labels are free-form strings sent verbatim.

**Jira-specific wrinkles:** the new-issue form's **issue type is required** (`create_issue` errors without one) and is populated from the project's `issueTypes`; `set_state` fetches the issue's workflow **transitions** and posts the one whose target `statusCategory` matches the wanted open/closed state (fails clearly if the workflow offers none); `supports_pr_summary = false`; milestones and GitHub-style projects stay empty; `rate_limit` is always `None` (Jira Cloud reports no counts). ADF flattening is intentionally lossy — tables, panels, and media are dropped.

**Verification gap:** no live Jira instance was available, so the REST endpoint shapes, ADF structure, and transition workflow are covered only by unit tests against sample payloads. (The Linear live drive caught two API bugs mocks would have missed — the same risk stands here until someone runs it against a real site.)

## Data fetch strategy

Org-wide issue listing uses the GraphQL `repositoryOwner(login:).repositories` connection (organisations and user accounts) with cursor pagination, fetching each repo's `issues` connection inline (first page) and following per-repo issue cursors where a repo has more than 100 issues.

The GitHub search API (`org:X is:issue`) was rejected because it silently caps at 1000 results. Repository iteration has no such cap.

Only open issues are fetched at startup (fast path). The first time the user cycles the state filter to closed/all, the dataset is upgraded once with a refetch that includes closed issues.

Including closed issues can push a single request's combined repo/issue page size over GitHub's GraphQL complexity budget (`Resource limits for this query exceeded`). `Client::graphql_with_backoff` catches this (`ProviderError::ResourceLimited`) and retries the same cursor with halved page sizes — cursors are opaque positions, so a smaller `first` mid-fetch is valid — down to a floor, after which the error is surfaced as readable text instead of a raw JSON dump.

## UI model

- **Row model**: the visible list is a flat `Vec<Row>` of `RepoHeader` and `Issue` entries, rebuilt (`rebuild_rows`) whenever data, filters, sort, or collapse state change. Selection is an index into this vector and is clamped on rebuild.
- **Collapse state** is a `HashSet<String>` of repo names, so it survives reloads and re-sorts.
- **Modes**: `Normal`, `Input(kind)` (single-line popup editor for search/filters/assignees/title/org), `IssueFormBody` (multi-line popup editor, `BodyEditor` key handling), `CommentEditor` (multi-line editor inline at the bottom of the detail pane rather than a popup — `Editor`/`Save`/`Cancel` sub-focus via `CommentFocus`, `Tab` cycles it, `Ctrl+S`/`Esc` work from any of the three), `PrioritySet` / `LabelsSet` (picker popups editing an existing issue's priority / label set, fed by `repo_labels`), `PrPicker` / `PrSummary` (`P` in the detail pane — picker over PR links found in the issue body/comments, then a summary popup fed by `Client::pull_request`), `FilterMenu`, `ConfirmState` (close/reopen confirmation popup with a `[ Yes ]  [ No ]` button row — `confirm_choice` on `App`, reset to `No` on open; `←`/`→`/`Tab`/`h`/`l` toggle focus, `Enter` picks, `y`/`n`/`Esc` remain direct shortcuts), `Help`.
- **Async**: all GitHub calls run in `tokio::spawn`ed tasks that report back over an mpsc channel (`AppEvent`); the event loop `select!`s over keys and app events. The UI never blocks on the network.
- **Consistency**: mutations trigger a full refetch on completion rather than optimistic patching — simpler, and correct by construction. When the detail pane is open, the same completion also refetches the open issue's comment thread, so a just-added comment appears without navigating away and back.
- **Two-region detail pane & inline editing**: the pane splits vertically (`detail_split`, body ≈45%) into a body region (issue metadata + description, scrolled by `body_scroll`) pinned above a comments region (comment cards — author · timestamp header rule, body, bottom rule — scrolled by `comments_scroll`). Each region draws its own right-edge `Scrollbar` when its content overflows. The selection is `DetailSel { Body, Comment(i) }` (`detail_sel`): `Tab`/`Shift+Tab` (`select_detail`, wrapping) move it and `←` returns focus to the list; `j`/`k` scroll only the selected region (`scroll_body` / `scroll_comment`), and selecting a comment snaps its header to the top (`snap_comment` + `comment_offset`) so scrolling then walks that one comment's own extent, its scrollbar tracking position within it. Wrapped heights are measured with ratatui's `Paragraph::line_count` (`unstable-rendered-line-info` feature) via shared `body_lines` / `comment_card_lines` builders, so measured and drawn geometry match exactly (the key handler recovers live dimensions from the terminal size in `detail_metrics`). `e` edits the selected region. The single `CommentEditor` mode serves add-comment, edit-comment, and edit-description via `EditorTarget` on `App`; `submit_comment` branches to the matching mutation and an empty save is discarded except for the description.

## Mutations

| Action | GraphQL |
|--------|---------|
| add comment | `addComment` |
| edit comment | `updateIssueComment(body:)` |
| edit description | `updateIssue(body:)` |
| close / reopen | `closeIssue` / `reopenIssue` |
| edit title | `updateIssue(title:)` |
| replace assignees | `user(login){id}` lookups, then `updateIssue(assigneeIds:)` |
| replace labels | `repository.labels` lookup, then `updateIssue(labelIds:)` |

The `IssueProvider::update_comment(issue_id, comment_id, body)` signature carries the issue id for every backend even though only Jira needs it (Jira's REST comment endpoint is `PUT /issue/{key}/comment/{id}`; GitHub/Linear address a comment by its node id alone). Linear maps the description edit to `issueUpdate(description:)` and Jira to a `PUT /issue/{key}` `fields.description` (ADF), so all three backends support editing.

Assignee/label edits are whole-set replacements. Assignees use a comma-separated text input pre-filled with the current set. Labels use a multi-select picker (same mechanics as the new-issue form's labels field) fed by `repository.labels`, pre-checked with the issue's current labels — Enter submits the checked set as the new full label set.

## Linked PR summaries

`P` in the detail pane scans the selected issue's body and loaded comment thread for explicit `github.com/{owner}/{repo}/pull/{N}` links (`parse_pr_links` in `provider/types.rs`) — bare `#N` is deliberately not matched, since in an issues tool it's ambiguous between an issue and a PR. Zero links reports a status message; one link fetches the summary directly; several open `Mode::PrPicker` first (reusing the filter-editor's picker machinery).

`Client::pull_request` fetches everything in one GraphQL query: title/body/state/draft, base/head refs, diffstat, `reviewDecision` plus the review list (deduped to each reviewer's latest state), comment/review-thread counts, the head commit's `statusCheckRollup` (the `CheckRun`/`StatusContext` union flattened into one DTO via `__typename`), the head commit's `checkSuites` (the PR's own Actions runs), and `defaultBranchRef`'s recent commits' `checkSuites` (the "merge to main" runs). `App::pr_target` guards the async response against a stale PR (the popup closed or retargeted before it landed) — the same pattern as `priority_pick_issue`/`label_pick_issue`.

## Security decisions

- Tokens never touch the config file; resolution is flag → env → `gh` CLI.
- The `Authorization` header is marked sensitive in reqwest (excluded from debug logs).
- TLS is rustls (no OpenSSL system dependency).
- Clipboard copy (`y`) is implemented via the OSC 52 terminal escape sequence written to stdout, not a system clipboard crate — keeps the "no system dependencies beyond a Rust toolchain" invariant and works over SSH (tmux passthrough handled).
- In the release workflow, `github.ref_name` is passed via `env:` rather than interpolated into the script.

## Testing

Pure logic lives in `tui/app.rs` and is covered by unit tests: filtering (state/text/repo/assignee/author/date bounds), sorting, grouping/collapse row building, selection clamping, UTF-8-safe input editing, date parsing, and PR-link collection/picker/stale-drop state. `github/client.rs` tests cover response DTO parsing (including deleted-author → `ghost`, pagination shapes, and `PrSummary` deserialisation across the `CheckRun`/`StatusContext` union and an empty rollup). `provider/types.rs` tests cover `parse_pr_links` (full URLs, dedup, trailing path/query, non-PR GitHub URLs, and rejecting bare `#N`). `github/auth.rs` tests the token chain with injected closures.

End-to-end verification for the initial release was done against the live `pgmac-net` org: initial load (106 issues / 18 repos), and a scripted keystroke session that added a comment to and closed a scratch issue through the TUI.
