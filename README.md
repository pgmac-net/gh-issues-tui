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
gh-issues --refresh 60          # auto-refresh every 60 seconds (0 disables)
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
refresh_interval = 300      # seconds between auto-refreshes, 0 disables (default: 300)
```

With `default_org` set, plain `gh-issues` works without `--org`. By default the issue list starts with every repo group folded; groups can still be expanded as normal (`Space` / `]`), and repos you expand stay expanded across reloads. When only one repo group is visible (for example when started inside a repo clone), that group starts expanded. Set `default_collapsed = false` to start with everything expanded. Tokens are never stored in the config file.

### Auto-refresh

The issue list refetches from GitHub every `refresh_interval` seconds (default 5 minutes) so new and updated issues appear without pressing `r`. The `--refresh <SECS>` flag overrides the config value; `0` disables it. A background refresh keeps your selection on the same issue and skips a beat while a fetch is already running, the API is rate-limited, or you are mid-edit (typing in an input, a menu, or a confirmation).

### Colour profiles

Define any number of `[color_profiles.<name>]` tables and pick one with `color_profile`:

```toml
color_profile = "gruvbox"

[color_profiles.gruvbox]
accent      = "#83a598"   # repo headers, prompts, help keys
dim         = "#928374"   # issue numbers, dates, metadata
selected_bg = "#3c3836"   # selection bar (list + pickers + calendar)
open        = "#b8bb26"   # open-issue dot and label
closed      = "#d3869b"   # closed-issue dot and label
assignee    = "#fabd2f"   # assignee badges / detail meta line
warning     = "#fe8019"   # rate-limit warnings, y/n prompts
error       = "#fb4934"   # errors
label_fallback = "blue"   # labels with unparsable GitHub colours

[color_profiles.mono]
accent = "white"
selected_bg = "8"
```

Every entry is optional — unset entries keep the built-in colour. Values accept ratatui colour names (`"cyan"`, `"lightgreen"`, `"dark gray"`), hex (`"#2d5aa0"`), or ANSI indexes (`"8"`). Naming a profile that isn't defined is a startup error listing the profiles that are.

## Keys

| Key | Action |
|-----|--------|
| `j`/`k`, `↑`/`↓` | move selection (scroll in detail view) |
| `PgUp`/`PgDn`, `g`/`G` | page / jump to top / bottom |
| `Space` | collapse/expand the selected repo group |
| `←` / `→` | on a repo header: collapse / expand the group. On an issue: `→` moves into the detail pane (opening it if closed), `←` backs out to the list |
| `[` / `]` | collapse all / expand all groups |
| `Enter` | open the issue in a right-hand detail pane (loads the comment thread) |
| `Tab` / `Shift+Tab` | switch focus between the list and detail panes |
| `Esc` / `q` | close the detail pane (from either pane) |
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
| `n` | create a new issue in the selected repo (opens the form) |
| `r` | reload all data |
| `?` | help |
| `q` | quit |

Sort keys: updated, created, closed, state, assignee, author.

### Creating issues

`n` opens a New-Issue form for the selected repo (from its header or any of its issue rows), modelled on GitHub's New Issue page: **title**, **description** (multi-line editor: Enter inserts a newline, Esc keeps the text and returns to the form), **assignees** and **labels** (multi-select pickers — Space toggles, Enter accepts), and **type**, **priority**, **project**, **milestone** (single-select pickers, `—` clears). Picker options load per repo when the form opens: assignable users, repo labels, issue types (where the org has them), the repo's Projects (V2), and open milestones. Priority follows the `priority:<value>` label convention — the chosen label is added to the issue's labels. `Enter` on `[ Create issue ]` submits; the status line reports `created #N` and the list refetches. `Esc` cancels the form.

Known limitation: repos with no issues are omitted from the fetched list entirely, so a repo's *first* issue can't be created from here yet.

## Notes

- Issues are fetched per-repository over the GraphQL API with cursor pagination, so organisations with more than 1000 issues are not truncated (the search API cap does not apply).
- Only open issues are fetched at startup unless `--all` is given; switching the state filter to closed/all triggers a one-time refetch that includes closed issues.
- Assignee and label edits replace the full set with what you type; comment/close/reopen/edit operations refresh the data on completion.
- In the filter editor, repo/assignee/author/priority/status open a picker built from the loaded data (first entry clears the filter) and date fields open a calendar; text remains free-input.
- Priority and status filters match `priority:<value>` / `status:<value>` labels (bare value or full label name, case-insensitive).
- The repo filter is exact when its text exactly names a loaded repo (case-insensitive), so `api` won't also match `api-gateway`; otherwise it matches as a substring.
- The detail pane splits the window 40/60 beside the list and live-follows the list selection: moving with `j`/`k` shows the newly selected issue (comments refetch per issue; landing on a repo header shows "no issue selected"). The focused pane has an accent-coloured border.
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
