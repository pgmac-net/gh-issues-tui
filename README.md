# gh-issues

Interactive TUI for browsing and managing GitHub issues across an entire organisation, written in Rust with [ratatui](https://ratatui.rs).

Issues from every repository in the organisation are listed in one place, grouped by repo with collapsible groups. Filter, sort, inspect, comment on, close/reopen, re-assign, re-label, re-title, and move issues to another repo — or jump out to the GitHub website — without leaving the terminal.

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
gh-issues --provider github     # issue backend (default: github; currently the only one)
```

`--org` accepts an organisation or a user account.

`--provider` selects the issue backend: `github` (default), `linear`, or `jira`.

### Linear backend

```sh
LINEAR_API_KEY=lin_api_… gh-issues --provider linear
```

Set `provider = "linear"` in the config to make it the default. Selection is explicit only — a git repo in the cwd never implies Linear.

Authentication uses a Linear **personal API key**: `--token` flag → `LINEAR_API_KEY` → `LINEAR_TOKEN`. As with GitHub, the key is never stored in the config file.

How Linear concepts map onto the UI:

| UI concept | Linear |
|---|---|
| Repo group | Team (shown by team key, e.g. `ENG`) |
| Issue number | Per-team issue number |
| `priority:*` label | Linear's native priority field, surfaced as a synthetic label so sort/colour/filter/`p` all work; setting it writes the native field back |
| Assignees | Single assignee (the list holds 0 or 1) |
| Close / reopen | Moves the issue to a `completed` / open workflow state in its team |
| Move issue (`m`) | Moves the issue to another team; Linear remaps its workflow state and label set to the target team itself |
| `--org` | Ignored — the workspace is fixed by the API key |

Not available on Linear: the linked-PR summary (`P`) — Linear issues have no GitHub PR links, so the key reports "not supported". Linear has no milestones or issue types in the GitHub sense; those form fields stay empty. The list view does not show a comment count for Linear issues (the count appears once you open the detail pane).

### Jira backend

```sh
JIRA_BASE_URL=https://your-site.atlassian.net \
JIRA_EMAIL=you@example.com \
JIRA_API_TOKEN=… \
gh-issues --provider jira
```

Jira **Cloud** only. Authentication is HTTP Basic with your email + an [API token](https://id.atlassian.com/manage-profile/security/api-tokens); all three env vars are required (`--token` overrides `JIRA_API_TOKEN`). Set `provider = "jira"` in the config to make it the default. As with the other backends, no secret is stored in the config file.

How Jira concepts map onto the UI:

| UI concept | Jira |
|---|---|
| Repo group | Project (shown by project key, e.g. `PROJ`) |
| Issue number | Numeric suffix of the issue key (`PROJ-123` → `123`) |
| `priority:*` label | Jira's native priority (`Highest`→urgent … `Low`/`Lowest`→low), surfaced as a synthetic label so sort/colour/filter/`p` all work; setting it writes the native field back |
| Body | Description is Atlassian Document Format (ADF); it's flattened to plain text for display and wrapped back to ADF when you create an issue or comment |
| Assignees | Single assignee (the list holds 0 or 1) |
| Close / reopen | Runs a workflow transition to a Done / not-Done status (needs a matching transition in the project's workflow) |
| Issue type | Required when creating an issue — the new-issue form's type picker is populated from the project |
| `--org` | Ignored — the site is fixed by the credentials |

Not available on Jira: the linked-PR summary (`P`, no GitHub PR links), milestones, and moving issues to another project (`m`) — Jira Cloud has no per-issue move, only a Beta bulk-move endpoint requiring an explicit issue-type/status mapping, so the key reports "not supported". Rich ADF formatting (tables, panels, media) is dropped when flattening to text.

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
provider = "github"         # issue backend (default: "github"; currently the only one)
default_collapsed = false   # start with repo groups expanded (default: true)
refresh_interval = 300      # seconds between auto-refreshes, 0 disables (default: 300)
hide_empty_repos = true     # hide repo groups with no visible issues (default: true)
copy_format = "{owner}/{repo}#{number}"   # `y` clipboard format (default shown)

default_harness = "claude"                # harness `A` launches (unset: `A` asks)
workspace_roots = ["~/pgmac", "~/projects"]   # searched for a repo's clone

[harnesses.claude]                        # built in; shown here to override it
command = ["claude", "/pgmac-workflows:pickup-ticket {ref}"]
```

With `default_org` set, plain `gh-issues` works without `--org`. By default the issue list starts with every repo group folded; groups can still be expanded as normal (`Space` / `]`), and repos you expand stay expanded across reloads. When only one repo group is visible (for example when started inside a repo clone), that group starts expanded. Set `default_collapsed = false` to start with everything expanded. Tokens are never stored in the config file.

`copy_format` controls what `y` puts on the clipboard, with `{owner}`, `{repo}`, and `{number}` placeholders substituted from the selected issue. The default (`{owner}/{repo}#{number}`) is the short form GitHub tools and Claude Code understand.

`default_harness`, `workspace_roots` and `[harnesses.*]` configure sending an issue to a coding agent — see [Coding harnesses](#coding-harnesses) below.

### Coding harnesses

`A` on an issue starts a coding agent in that repo's clone, primed with the ticket, in an embedded terminal pane. Full detail: [`docs/harness-sessions.md`](docs/harness-sessions.md).

```toml
default_harness = "claude"
workspace_roots = ["~/pgmac", "~/projects"]

[harnesses.claude]
command = ["claude", "/pgmac-workflows:pickup-ticket {ref}"]

[harnesses.opencode]
command = ["opencode", "run", "work on {url}"]
```

`command` is an **argv array, never a shell string** — placeholders expand into individual arguments, so issue text containing quotes or `$(…)` is inert. Available placeholders are `{owner}`, `{repo}`, `{number}`, `{ref}` (`owner/repo#number`, always canonical) and `{url}`.

`claude` and `opencode` ship built in; defining a harness of the same name overrides it, and defining a new one leaves the others in place. A harness may set its own `workspace_roots`.

Other agents are not shipped as defaults because their argument forms were not verified — a guessed argv fails at spawn time. Check their `--help`, then paste:

```toml
[harnesses.codex]
command = ["codex", "work on {url}"]

[harnesses.copilot]
command = ["copilot", "-p", "work on {ref}"]

[harnesses.pi]
command = ["pi", "{ref}"]
```

A harness runs in the current directory's repo when that is the issue's repo; otherwise in the first `<root>/<repo>` that exists across `workspace_roots`. If none does, nothing launches and the message names every path tried.

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
code_bg     = "#1d2021"   # fenced/inline code background
code_fg     = "#ebdbb2"   # fenced/inline code text

[color_profiles.mono]
accent = "white"
selected_bg = "8"
```

Every entry is optional — unset entries keep the built-in colour. Values accept ratatui colour names (`"cyan"`, `"lightgreen"`, `"dark gray"`), hex (`"#2d5aa0"`), or ANSI indexes (`"8"`). Naming a profile that isn't defined is a startup error listing the profiles that are.

## Keys

| Key | Action |
|-----|--------|
| `j`/`k`, `↑`/`↓` | move selection in the list; in the detail pane, scroll the selected region (issue body or the selected comment) |
| `PgUp`/`PgDn`, `g`/`G` | page / jump to top / bottom |
| `Space` | collapse/expand the selected repo group |
| `←` / `→` | on a repo header: collapse / expand the group. On an issue: `→` moves into the detail pane (opening it if closed), `←` backs out to the list |
| `[` / `]` | collapse all / expand all groups |
| `Enter` | open the issue in a right-hand detail pane (loads the comment thread) |
| `Tab` / `Shift+Tab` | from the list: switch into the detail pane. In the detail pane: move to the next / previous comment |
| `←` | in the detail pane: return focus to the list |
| `Esc` / `q` | close the detail pane (from either pane) |
| `o` / `O` | open issue / repo in the browser |
| `y` | copy the selected issue's short reference (`owner/repo#number`) to the clipboard, via OSC 52 |
| `/` | free-text search (title, body, `#number`) |
| `#` | jump the selection to a loaded issue by number (moves the selector bar; does not filter — reveals a hidden match by clearing filters and relaxing the state filter, searches the current repo group first) |
| `f` | cycle state filter: open → closed → all |
| `F` | filter editor (repo, assignee, author, priority, status, created/updated/closed date bounds) |
| `s` / `S` | cycle sort key / toggle direction |
| `w` | switch org/owner (free-text; resets filters and view state) |
| `c` | add a comment: opens the detail pane (if closed) and its inline editor section (`Ctrl+S` submits, `Esc` discards) |
| `e` | edit the selected detail region inline — the issue description (body) or the selected comment — in the same editor section |
| `x` | close or reopen the issue (confirmation popup: `←`/`→`/`Tab` moves focus, `Enter` picks, or `y`/`n`/`Esc` shortcuts) |
| `a` | edit assignees (comma-separated logins) |
| `l` | edit labels (picker of the repo's labels, current labels pre-checked) |
| `t` | edit the title |
| `p` | set the priority (picker of the repo's `priority:*` labels, `—` clears) |
| `P` | summarise a linked PR (detail pane only; picker if several links are found) |
| `m` | move the issue to another repo (picker of the org's other repos, then a confirmation popup) |
| `A` | send the issue to a coding harness — starts it in that repo's clone in an embedded terminal pane; attaches if the issue already has a session |
| `Z` | switch between harness sessions |
| `n` | create a new issue in the selected repo (opens the form) |
| `r` | reload all data |
| `?` | help |
| `q` | quit |

Sort keys: updated, created, closed, state, assignee, author, priority.

### Editing keys

Every text input (search, filters, assignees, title, org, and the comment/description editor) opens as a small popup box and supports readline-style editing; the new-issue form's title and description fields use the same editing keys but inline, in the form itself. The cursor is a block sitting **on** a character:

| Key | Action |
|-----|--------|
| `←`/`→`, `Home`/`End` | move by char / to line start / to line end |
| `Ctrl+←` / `Ctrl+→` | move left / right by word (whitespace-delimited) |
| `Ctrl+A` / `Ctrl+E` | line start / line end |
| `Ctrl+W` | delete the word before the cursor |
| `Ctrl+U` / `Ctrl+K` | delete to line start / to line end |
| `Ctrl+D` / `Delete` | delete the char under the cursor |

Single-line popups (search, filters, assignees, title, org) and the new-issue form's inline title field scroll horizontally to keep the cursor visible when the value is wider than the box; `Enter` submits (on the new-issue title, `Enter` moves focus to the next field instead), `Esc` cancels.

In the multi-line comment and description editors — including the new-issue form's inline description — text word-wraps at the editor's width, `↑`/`↓` move by *visual* (wrapped) row, `Enter` inserts a newline, and `Delete` at the end of a line joins the next line on.

### Adding a comment

`c` adds a comment on the selected issue: it opens the detail pane if it's closed (loading the comment thread) and adds an inline section at the bottom, about a third of the pane's height, with a multi-line editor and a `[ Save ]  [ Cancel ]` button row. `Tab`/`Shift+Tab` cycle focus between the editor and the two buttons; `Enter`/`Space` activates whichever is focused. `Ctrl+S` saves and `Esc` cancels from anywhere in the section, regardless of focus. Saving posts the comment and refreshes the thread; cancelling discards the draft and returns to the plain detail pane.

### Editing the description and comments

The detail pane splits into two independently scrolling regions. The **top region** pins the issue metadata and description so it never scrolls away with the comments. The **bottom region** holds the comment thread, rendered as **cards** — each comment is bounded by a header rule showing its author and timestamp and a matching bottom rule, so where one comment ends and the next begins is obvious. Each region shows a scrollbar on its right edge when its content is taller than the region, marking your position through the text.

`Tab` / `Shift+Tab` move the selection through the body and each comment (wrapping around); `←` hands focus back to the list. `j`/`k` (or `↑`/`↓`) scroll the **selected** region only — the description when the body is selected, or, once you select a comment, that comment's own text (its header scrolls up out of the way as you read, and the scrollbar tracks your position within that one comment). `PageUp`/`PageDown` step by a screenful. `e` opens the same inline editor section on the selected region, prefilled with its current text — the issue **description** when the body is selected, or the **comment** otherwise. Editing uses the identical editor controls as adding a comment (`Ctrl+S` saves, `Esc` discards). Saving an edited description or comment refreshes the pane; an empty save discards a comment edit but is accepted for the description (clearing it is valid). All three backends (GitHub, Linear, Jira) support editing.

The body and each comment render as lightweight markdown: `# ` headings, `**bold**`/`*italic*`, `` `inline code` ``, fenced ` ``` ` code blocks, `> ` blockquotes, `-`/`*`/`+` and numbered lists, `---` rules, and `[text](url)` links. Rendering is line-for-line — one screen line per source line — with wrapping measured accurately so scroll positions and scrollbars stay correct; tables, nested-list re-indent, and syntax highlighting inside code fences aren't supported.

URLs in the description and comments — both bare `http(s)://` URLs and the labels of `[text](url)` links — are **clickable**. They're emitted as terminal OSC 8 hyperlinks, so **Ctrl+Click** (**Cmd+Click** on macOS) opens them in your default browser. This uses no mouse capture, so ordinary mouse text-selection still works; terminals that don't support OSC 8 simply show the URLs as plain underlined text.

### Creating issues

`n` opens a New-Issue form for the selected repo (from its header or any of its issue rows) as a single inline form — one box, no per-field popups except for the pickers below. `Tab`/`Shift+Tab` move between fields and the `[ Create ]`/`[ Cancel ]` buttons at the bottom, wrapping at both ends. **Title** and **description** edit directly in the form (description is a small multi-line box: `Enter` inserts a newline; `Tab` leaves it). **Assignees** and **labels** (multi-select pickers — Space toggles, Enter accepts) and **type**, **priority**, **project**, **milestone** (single-select pickers, `—` clears) still open a picker popup on `Enter` — the one modal exception, since these option lists benefit from the pickers' type-ahead filter. Picker options load per repo when the form opens: assignable users, repo labels, issue types (where the org has them), the repo's Projects (V2), and open milestones. Priority follows the `priority:<value>` label convention — the chosen label is added to the issue's labels. `Enter`/`Space` on `[ Create ]` submits; the status line reports `created #N` and the list refetches. `Esc` anywhere, or `Enter`/`Space` on `[ Cancel ]`, discards the form.

To create the *first* issue in a repo that shows no issues, flip the `hide empty repos` filter to `no` (`F` → last row → Enter) — the repo's `(0)` header appears and `n` works on it.

### Moving an issue to another repo

`m` moves the selected issue to a different repo in the same org: it opens a picker of every other loaded repo (type-ahead filters it), then a confirmation popup naming the destination and the move's side effects — the issue gets a new number, and everyone mentioned in it is notified. `Esc`/`n` cancels either step without mutating anything.

GitHub only allows same-owner transfers — the picker's targets are exactly the repos already loaded for this org, so there's nothing to fetch. Labels missing from the target repo are recreated there (`priority:*`/`status:*` included) so the app's sort/colour/filter keeps working on the moved issue; recreated labels get GitHub's default colour rather than the source's. On Linear, `m` moves the issue to another team instead — see the [Linear backend](#linear-backend) table for what that remaps. Not available on Jira — see the [Jira backend](#jira-backend) section.

The selection does not follow the moved issue after the refetch — GitHub does not document whether a transferred issue keeps its id, so the cursor falls back to the same clamped-index behaviour as any issue that vanishes from view. The status line names the destination instead (`moved to <repo>`).

### Linked PR summaries

With the detail pane open, `P` scans the issue's body and its loaded comment thread for references in three forms: `github.com/<owner>/<repo>/pull/<N>` links, `<owner>/<repo>#<N>` shorthand, and bare `#<N>` (resolved against the repo the issue belongs to). One reference opens its summary directly; several open a picker to choose. The summary popup shows the PR's title/description, state (open/closed/merged/draft), base←head branches and diffstat, review status (GitHub's overall decision plus per-reviewer approve/changes-requested/comment counts), issue-comment and review-thread counts, the head commit's checks, the PR's own Actions runs, and recent Actions runs on the repo's default branch (the "merge to main" runs). `j`/`k` scroll it; `Esc`/`q` closes back to the detail pane.

The shorthands cannot say whether a number is a PR or an issue — GitHub draws both from one per-repo sequence — so a candidate is only resolved when you open it. If it turns out to be an issue, the popup says so and `o`/Enter jumps the selector to it, falling back to opening it in a browser when it isn't in the loaded data. References inside code fences and inline code spans are ignored. See `docs/pr-url-matching.md`.

### Harness sessions

`A` starts the configured coding agent for the selected issue, in that repo's clone, on a real PTY drawn full-frame inside the TUI. Several sessions run at once, one per issue.

Inside a session **every key goes to the agent** — arrows, `Esc`, `Ctrl+C` and `Shift+Tab` included, since agent CLIs bind them. `F12` is the single key the TUI keeps back, as a tmux-style prefix:

| Chord | Effect |
|---|---|
| `F12 d` | detach — back to the list, the agent keeps running |
| `F12 s` | switch session |
| `F12 k` | kill (confirmed while the agent is alive) |
| `F12 n` | start another harness for the current issue |
| `F12 ?` | help |
| `F12 F12` | send a literal `F12` to the agent |

`F12 ?` shows that table in-app. Help opened from inside a session lists these chords; `?` from the issue list keeps the list's own keys. The two tables are disjoint on purpose — a session forwards `n` and `/` to the agent, so offering them as app keys there would be actively wrong.

Detaching is not killing: the agent keeps working and `Z` returns to it. When an agent exits, its session stays with the final screen frozen so the closing summary is still readable — `k`/`j` scroll, `G` jumps to the end, `q` leaves it in place, `x` dismisses it. The bottom status line shows `n running, m exited (Z)` whenever any session exists, and `q` with agents still running asks first, naming them.

#### Telling a session apart from a normal terminal

An agent's own TUI fills the screen, so a session carries two rows of chrome that belong to the app rather than the agent:

```
█ gh-issues-tui █ pgmac-net/gh-issues-tui#132 · Send to Claude/harness · claude · ● running
  (the agent's own full-screen UI)
 F12 d detach · s switch · k kill · n new · F12 F12 literal · ? help
```

The top row is the identity row: a reverse-video badge, the issue, its title, which harness, and whether the child is alive. It costs the agent one row of its PTY, so on a very short terminal it is shed before the key row is. Under width pressure the title elides first, then the owner drops off the reference (`…/gh-issues-tui#132`); the badge and the running state never elide.

The window/tab title is set to `gh-issues-tui · <issue>` while attached and back to `gh-issues-tui` on detach, so provenance survives the TUI not being the focused pane. An agent that emits its own title escape will win — the identity row is the signal that cannot be overwritten.

#### Environment passed to the agent

Every harness child gets these on top of the inherited environment:

| Variable | Example |
|---|---|
| `GH_ISSUES_TUI` | `0.12.0` — presence marks the launcher, the value is its version |
| `GH_ISSUES_TUI_HARNESS` | `claude` |
| `GH_ISSUES_TUI_OWNER` | `pgmac-net` |
| `GH_ISSUES_TUI_REPO` | `gh-issues-tui` |
| `GH_ISSUES_TUI_NUMBER` | `132` |
| `GH_ISSUES_TUI_ISSUE` | `pgmac-net/gh-issues-tui#132` |
| `GH_ISSUES_TUI_URL` | `https://github.com/pgmac-net/gh-issues-tui/issues/132` |
| `GH_ISSUES_TUI_TITLE` | `Send to Claude/harness` |

The agent itself already learns its ticket from the prompt in its `command`; these are for everything that cannot read that — hooks, statuslines and scripts, including ones the agent spawns. Scraping the parent's argv instead does not generalise: every harness formats its command differently, and the `opencode` default carries a URL with no `#N` in it at all.

To render provenance inside Claude Code's own UI, in `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "[ -n \"$GH_ISSUES_TUI\" ] && echo \"⬡ gh-issues-tui · $GH_ISSUES_TUI_ISSUE\" || true"
  }
}
```

Full detail: [`docs/harness-sessions.md`](docs/harness-sessions.md).

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
