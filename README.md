# gh-issues

Interactive TUI for browsing and managing GitHub issues across an entire organisation, written in Rust with [ratatui](https://ratatui.rs).

Issues from every repository in the organisation are listed in one place, grouped by repo with collapsible groups. Filter, sort, inspect, comment on, close/reopen, re-assign, re-label and re-title issues — or jump out to the GitHub website — without leaving the terminal.

## Install

Download a binary from the [releases page](https://github.com/pgmac-net/gh-issues-tui/releases), or build from source:

```sh
cargo build --release
# binary at target/release/gh-issues
```

## Usage

```sh
gh-issues --org my-org          # open issues only (default)
gh-issues --org my-org --all    # include closed issues in the initial fetch
gh-issues                       # inside a repo clone: that repo's owner, filtered to the repo
```

`--org` accepts an organisation or a user account.

### Starting inside a repository clone

When run from a directory inside a git repository whose `origin` remote points at github.com, `gh-issues` browses that remote's owner with the repo filter pre-set to the repository — so you see just that repo's issues immediately. Clear the filter (`F` → `c`, or empty the repo field) to see the whole organisation again.

Resolution order for what to browse:

1. `--org` flag (the detected repo filter is applied only when the remote's owner matches)
2. the cwd's `origin` remote owner + repo filter
3. `default_org` from the config file

### Authentication

A GitHub token is resolved in this order:

1. `--token <TOKEN>` flag
2. `GITHUB_TOKEN` environment variable
3. `GH_TOKEN` environment variable
4. `gh auth token` (the GitHub CLI's stored login)

On a machine with `gh` logged in, no configuration is needed. The token needs `repo` scope (read for browsing; write operations use the same token).

### Configuration

Optional TOML config at `~/.config/gh-issues/config.toml`:

```toml
default_org = "my-org"
default_collapsed = false   # start with repo groups expanded (default: true)
```

With `default_org` set, plain `gh-issues` works without `--org`. By default the issue list starts with every repo group folded; groups can still be expanded as normal (`Space` / `]`), and repos you expand stay expanded across reloads. When only one repo group is visible (for example when started inside a repo clone), that group starts expanded. Set `default_collapsed = false` to start with everything expanded. Tokens are never stored in the config file.

## Keys

| Key | Action |
|-----|--------|
| `j`/`k`, `↑`/`↓` | move selection (scroll in detail view) |
| `PgUp`/`PgDn`, `g`/`G` | page / jump to top / bottom |
| `Space` | collapse/expand the selected repo group |
| `←` / `→` | collapse / expand the selected repo group (`←` in detail view backs out) |
| `[` / `]` | collapse all / expand all groups |
| `Enter` | open issue detail (loads the comment thread) |
| `Esc` / `q` | back out of detail view |
| `o` / `O` | open issue / repo in the browser |
| `/` | free-text search (title, body, `#number`) |
| `f` | cycle state filter: open → closed → all |
| `F` | filter editor (repo, assignee, author, priority, status, created/updated/closed date bounds) |
| `s` / `S` | cycle sort key / toggle direction |
| `w` | switch org/owner (free-text; resets filters and view state) |
| `c` | add a comment |
| `x` | close or reopen the issue (asks y/n) |
| `a` | edit assignees (comma-separated logins) |
| `l` | edit labels (comma-separated names, must exist on the repo) |
| `t` | edit the title |
| `r` | reload all data |
| `?` | help |
| `q` | quit |

Sort keys: updated, created, closed, state, assignee, author.

## Notes

- Issues are fetched per-repository over the GraphQL API with cursor pagination, so organisations with more than 1000 issues are not truncated (the search API cap does not apply).
- Only open issues are fetched at startup unless `--all` is given; switching the state filter to closed/all triggers a one-time refetch that includes closed issues.
- Assignee and label edits replace the full set with what you type; comment/close/reopen/edit operations refresh the data on completion.
- In the filter editor, repo/assignee/author/priority/status open a picker built from the loaded data (first entry clears the filter) and date fields open a calendar; text remains free-input.
- Priority and status filters match `priority:<value>` / `status:<value>` labels (bare value or full label name, case-insensitive).
- The repo filter is exact when its text exactly names a loaded repo (case-insensitive), so `api` won't also match `api-gateway`; otherwise it matches as a substring.
- The info bar shows the API rate-limit budget (`API remaining/limit`); after a mutation the refetch is skipped if the budget is exhausted, and rate-limit errors stay visible until a fetch succeeds.

## Development

```sh
cargo test                     # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

See [docs/](docs/) for architecture and design notes.

## License

MIT
