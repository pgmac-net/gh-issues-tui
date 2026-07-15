# gh-issues — architecture and design notes

Built for [pgmac-net/homelabia#128](https://github.com/pgmac-net/homelabia/issues/128).

## Overview

`gh-issues` is a single-binary Rust TUI (ratatui + crossterm + tokio) that browses and manages GitHub issues across a whole organisation. The module layout deliberately mirrors [docker-registry-walk](https://github.com/pgmac-net/docker-registry-walk), the org's existing Rust TUI.

```
src/
├── main.rs          CLI (clap), panic hook, wiring
├── config.rs        ~/.config/gh-issues/config.toml (default_org)
├── github/
│   ├── auth.rs      token chain: --token → GITHUB_TOKEN → GH_TOKEN → gh auth token
│   ├── client.rs    GraphQL v4 client: org-wide fetch + mutations
│   ├── types.rs     Issue / RepoIssues / Comment / IssueState
│   └── error.rs     thiserror error type
└── tui/
    ├── app.rs       all state + pure logic (filters, sort, rows, input)
    ├── event.rs     async event loop, background tasks
    └── ui.rs        pure render
```

## Data fetch strategy

Org-wide issue listing uses the GraphQL `repositoryOwner(login:).repositories` connection (organisations and user accounts) with cursor pagination, fetching each repo's `issues` connection inline (first page) and following per-repo issue cursors where a repo has more than 100 issues.

The GitHub search API (`org:X is:issue`) was rejected because it silently caps at 1000 results. Repository iteration has no such cap.

Only open issues are fetched at startup (fast path). The first time the user cycles the state filter to closed/all, the dataset is upgraded once with a refetch that includes closed issues.

## UI model

- **Row model**: the visible list is a flat `Vec<Row>` of `RepoHeader` and `Issue` entries, rebuilt (`rebuild_rows`) whenever data, filters, sort, or collapse state change. Selection is an index into this vector and is clamped on rebuild.
- **Collapse state** is a `HashSet<String>` of repo names, so it survives reloads and re-sorts.
- **Modes**: `Normal`, `Input(kind)` (single-line popup editor for search/filters/assignees/title/org), `CommentEditor` / `IssueFormBody` (multi-line popup editors sharing the same `BodyEditor` key handling), `PrioritySet` / `LabelsSet` (picker popups editing an existing issue's priority / label set, fed by `repo_labels`), `FilterMenu`, `ConfirmState` (y/n for close/reopen), `Help`.
- **Async**: all GitHub calls run in `tokio::spawn`ed tasks that report back over an mpsc channel (`AppEvent`); the event loop `select!`s over keys and app events. The UI never blocks on the network.
- **Consistency**: mutations trigger a full refetch on completion rather than optimistic patching — simpler, and correct by construction.

## Mutations

| Action | GraphQL |
|--------|---------|
| add comment | `addComment` |
| close / reopen | `closeIssue` / `reopenIssue` |
| edit title | `updateIssue(title:)` |
| replace assignees | `user(login){id}` lookups, then `updateIssue(assigneeIds:)` |
| replace labels | `repository.labels` lookup, then `updateIssue(labelIds:)` |

Assignee/label edits are whole-set replacements. Assignees use a comma-separated text input pre-filled with the current set. Labels use a multi-select picker (same mechanics as the new-issue form's labels field) fed by `repository.labels`, pre-checked with the issue's current labels — Enter submits the checked set as the new full label set.

## Security decisions

- Tokens never touch the config file; resolution is flag → env → `gh` CLI.
- The `Authorization` header is marked sensitive in reqwest (excluded from debug logs).
- TLS is rustls (no OpenSSL system dependency).
- In the release workflow, `github.ref_name` is passed via `env:` rather than interpolated into the script.

## Testing

Pure logic lives in `tui/app.rs` and is covered by unit tests: filtering (state/text/repo/assignee/author/date bounds), sorting, grouping/collapse row building, selection clamping, UTF-8-safe input editing, and date parsing. `github/client.rs` tests cover response DTO parsing (including deleted-author → `ghost`) and pagination shapes. `github/auth.rs` tests the token chain with injected closures.

End-to-end verification for the initial release was done against the live `pgmac-net` org: initial load (106 issues / 18 repos), and a scripted keystroke session that added a comment to and closed a scratch issue through the TUI.
