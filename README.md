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
hide_empty_repos = true     # hide repo groups with no visible issues (default: true)
copy_format = "{owner}/{repo}#{number}"   # `y` clipboard format (default shown)
```

With `default_org` set, plain `gh-issues` works without `--org`. By default the issue list starts with every repo group folded; groups can still be expanded as normal (`Space` / `]`), and repos you expand stay expanded across reloads. When only one repo group is visible (for example when started inside a repo clone), that group starts expanded. Set `default_collapsed = false` to start with everything expanded. Tokens are never stored in the config file.

`copy_format` controls what `y` puts on the clipboard, with `{owner}`, `{repo}`, and `{number}` placeholders substituted from the selected issue. The default (`{owner}/{repo}#{number}`) is the short form GitHub tools and Claude Code understand.

### Auto-refresh

The issue list refetches from GitHub every `refresh_interval` seconds (default 5 minutes) so new and updated issues appear without pressing `r`. The `--refresh <SECS>` flag overrides the config value; `0` disables it. A background refresh keeps your selection on the same issue and skips a beat while a fetch is already running, the API is rate-limited, or you are mid-edit (typing in an input, a menu, or a confirmation).

### Clipboard

`y` copies the selected issue's short reference via an [OSC 52](https://www.reddit.com/r/vim/comments/k1ydpn/a_guide_on_how_to_copy_text_from_anywhere/) terminal escape sequence rather than talking to a system clipboard library, so it works the same locally and over SSH (tmux passthrough is handled automatically). It needs a terminal emulator with OSC 52 support — true of most modern terminals (iTerm2, Alacritty, Kitty, WezTerm, Windows Terminal) — and, if you're not local, an SSH client/terminal combination that lets OSC 52 through.

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
| `y` | copy the selected issue's short reference (`owner/repo#number`) to the clipboard, via OSC 52 |
| `/` | free-text search (title, body, `#number`) |
| `f` | cycle state filter: open → closed → all |
| `F` | filter editor (repo, assignee, author, priority, status, created/updated/closed date bounds) |
| `s` / `S` | cycle sort key / toggle direction |
| `w` | switch org/owner (free-text; resets filters and view state) |
| `c` | add a comment (multi-line editor; `Ctrl+S` submits, `Esc` discards) |
| `x` | close or reopen the issue (asks y/n) |
| `a` | edit assignees (comma-separated logins) |
| `l` | edit labels (picker of the repo's labels, current labels pre-checked) |
| `t` | edit the title |
| `p` | set the priority (picker of the repo's `priority:*` labels, `—` clears) |
| `n` | create a new issue in the selected repo (opens the form) |
| `r` | reload all data |
| `?` | help |
| `q` | quit |

Sort keys: updated, created, closed, state, assignee, author, priority.

### Editing keys

Every text input (search, filters, assignees, title, org, the new-issue title, and the comment/description editors) opens as a small popup box and supports readline-style editing. The cursor is a block sitting **on** a character:

| Key | Action |
|-----|--------|
| `←`/`→`, `Home`/`End` | move by char / to line start / to line end |
| `Ctrl+←` / `Ctrl+→` | move left / right by word (whitespace-delimited) |
| `Ctrl+A` / `Ctrl+E` | line start / line end |
| `Ctrl+W` | delete the word before the cursor |
| `Ctrl+U` / `Ctrl+K` | delete to line start / to line end |
| `Ctrl+D` / `Delete` | delete the char under the cursor |

Single-line popups (search, filters, assignees, title, org, new-issue title) scroll horizontally to keep the cursor visible when the value is wider than the box; `Enter` submits, `Esc` cancels.

In the multi-line comment and description editors, text word-wraps at the popup width, `↑`/`↓` move by *visual* (wrapped) row, `Enter` inserts a newline, and `Delete` at the end of a line joins the next line on. The comment editor submits on `Ctrl+S` (`Esc` discards); the new-issue description keeps its text on `Esc` and returns to the form.

### Creating issues

`n` opens a New-Issue form for the selected repo (from its header or any of its issue rows), modelled on GitHub's New Issue page: **title**, **description** (multi-line editor: Enter inserts a newline, Esc keeps the text and returns to the form), **assignees** and **labels** (multi-select pickers — Space toggles, Enter accepts), and **type**, **priority**, **project**, **milestone** (single-select pickers, `—` clears). Picker options load per repo when the form opens: assignable users, repo labels, issue types (where the org has them), the repo's Projects (V2), and open milestones. Priority follows the `priority:<value>` label convention — the chosen label is added to the issue's labels. `Enter` on `[ Create issue ]` submits; the status line reports `created #N` and the list refetches. `Esc` cancels the form.

To create the *first* issue in a repo that shows no issues, flip the `hide empty repos` filter to `no` (`F` → last row → Enter) — the repo's `(0)` header appears and `n` works on it.

## Notes

- Issues are fetched per-repository over the GraphQL API with cursor pagination, so organisations with more than 1000 issues are not truncated (the search API cap does not apply).
- Only open issues are fetched at startup unless `--all` is given; switching the state filter to closed/all triggers a one-time refetch that includes closed issues.
- Assignee edits replace the full set with what you type; label edits replace the full set with what's checked in the picker; comment/close/reopen/edit operations refresh the data on completion. With the detail pane open, the comment thread refreshes too, so a just-added comment appears immediately without moving the selection.
- `p` fetches the repo's labels and offers the `priority:*` ones (ordered low → urgent, current priority pre-highlighted). Picking replaces any existing priority label and keeps the rest; `—` removes the priority. Repos with no `priority:*` labels report that in the status line instead of opening the picker.
- `l` fetches the repo's labels and offers all of them as a multi-select (Space toggles, Enter accepts), with the issue's current labels pre-checked. Accepting replaces the issue's full label set with the checked ones. Repos with no labels report that in the status line instead of opening the picker.
- In the filter editor, repo/assignee/author open a single-select picker built from the loaded data (first entry clears the filter); priority/status open a multi-select picker (Space toggles, Enter accepts, deselecting everything clears the filter — priority options ordered low → urgent); date fields open a calendar; text remains free-input.
- Repo groups with zero visible issues are hidden by default. The `hide empty repos` row in the filter editor toggles this in place (Enter flips yes/no): set to `no`, every repo appears — including repos with no issues at all and groups emptied by the current filters — as a `(0)` header. Clearing filters (`F` → `c`) and switching org reset the toggle to the `hide_empty_repos` config default. Archived repos and repos with issues disabled are never shown.
- Every option picker (filter editor and new-issue form) supports type-ahead: just start typing to narrow the list (case-insensitive substring, shown as a `/ <text>` row). `Backspace` edits the filter, `Ctrl+U` clears it, `↑`/`↓` navigate the matches, `Enter` picks, `Esc` closes. Because typing filters, `j`/`k`/`q` don't navigate/close inside pickers.
- Priority and status filters match `priority:<value>` / `status:<value>` labels (bare value or full label name, case-insensitive). Several values can be selected at once — an issue matches when it carries any of them.
- Issues carrying a `priority:<value>` label have their title drawn in that label's GitHub colour (in both the list and the detail pane); issues without one keep the default colour. The first priority label wins if an issue somehow has several.
- The `priority` sort key ranks urgent > high > medium > low > no priority (descending shows urgent first). Priority values other than those four sort with the no-priority group, and equal priorities order by most recently updated regardless of sort direction.
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
