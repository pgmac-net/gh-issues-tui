# In-app help (`#184`)

Ticket: [pgmac-net/gh-issues-tui#184](https://github.com/pgmac-net/gh-issues-tui/issues/184)
· Related: [semantic search](semantic-search.md) (#158), [ticket readiness](ticket-readiness.md) (#160), [priority-rank inference](priority-rank-inference.md) (#156)

## What it does

`?` (from the issue list) or **`F1`** (from anywhere else — including while
typing, and over any popup) opens a help viewer: a strip of five pages across
the top, a live status block, then the page's text. `←`/`→` or `Tab`/
`Shift+Tab` switch page; `j`/`k`/`↑`/`↓`/`PageUp`/`PageDown` scroll; `Esc`,
`q`, `?` or `F1` close it and return to **exactly** where you were — a
half-typed search included.

## Why `F1`, and not just `?`

`?` is a character. In every text input — `/`, the filter editor's fields,
the title editor, the comment editor — typing `?` had to keep typing `?`, so
help there needed a key nothing else claims. `F1` is free everywhere in this
app and is the conventional help key, so it opens help from every mode except
`Mode::Harness`, where every key belongs to the coding agent. `?` keeps its
old, narrower meaning in exactly two places: the list, and `F12 ?` inside a
session, which still shows the short session-key table, unchanged.

## Which page opens, and why

`App::context_topic` is a pure function of `Mode` (plus, for the list, the
detail pane's focus and whether a readiness verdict has landed):

| context | opens | why |
|---|---|---|
| the list | keys | nothing more specific applies |
| detail pane focused, a readiness badge shown | readiness | you are looking straight at it |
| detail pane focused, no badge | keys | nothing to explain yet |
| `/` input; filter editor's `text` row | search | the two places that drive it |
| filter editor's `priority` row; the `p` picker; its confirmation | priority | the write path this feature touches |
| everywhere else | keys | never nothing |

Anywhere not listed falls through to `keys`, so `F1` always shows something.
The table names filter rows by index (`FILTER_FIELDS[0]`, `[4]`); a pinned
test fails if the editor's rows are ever reordered, rather than `F1` silently
opening the wrong page from then on.

## The viewer is not a modal

Before this, `Mode::Help` closed on *any* key — fine for one short table, not
for five pages of prose someone might want to read and scroll. `Mode::Help`
now carries a `HelpTopic`, and `App.help.return_to` records the mode help was
opened from. That assignment happens once, on the transition **into** help —
switching pages while already inside it must never make help its own return
point, or closing would land you in help. A test pins this by opening help,
switching pages, then closing, and checking the mode is the one from before
either happened.

`should_auto_refresh` treats help opened over the passive list the same as
the list itself, and help opened over anything else (a popup, a half-typed
input) the same as that — a refresh would otherwise replace data sitting
under an open popup.

## The status block

Each feature page opens with a line built from `typesafe::Status` — the two
config flags and whether `TYPESAFE_API_KEY` is present, **never its value**:

```
semantic search   off — send_issue_text is false
```

`typesafe` gets all three plus the key line. The wording names whichever is
actually missing — the flag first, since that is the choice the user makes,
then the key — and, once both are set, whether a request has since failed
this session and how it recovers (search and priority ranks: switching org;
the readiness badge: the next refresh). `App::help_status` is the only thing
that may say a feature is "on", and a test pins it to agree with
`Client::from_settings` about what "on" means, so the two cannot drift apart.

The block is generated, never baked into the markdown, so the same page is
correct for a reader with different settings from yours.

## The semantic-search indicator

The same live-status idea, but in the info bar, not a help page — because it
answers "is this working *right now*", which only matters while you are
typing a search. See [`semantic-search.md`](semantic-search.md#telling-whether-it-is-on).

## Keeping the pages honest

**The keys page is generated**, from the same `LIST_HELP` table the app has
always drawn — so it is the one page that structurally cannot drift.

The other four are `docs/help/*.md`, loaded with `include_str!` at compile
time, so the app renders the exact bytes committed to the repo and GitHub
shows the exact page the app displays — there is no second copy to fall out
of sync with the first.

**Their *content* can still drift from the code**, so `tui/app/help.rs`'s
tests read each page back and check it against the constants and settings
the code actually uses:

- `search.md` states `SEARCH_YES` and `BODY_CHARS`, and the exact wording the
  info bar uses (`semantic: searching…`, `+`, `off (`).
- `readiness.md` states the body, comment-count and comment-length limits,
  and every verdict's leading word.
- `priority.md` states `MIN_CONFIDENCE` and the cache file's name.
- Every setting any page shows in a config block is checked against a real
  `Config`, parsed from that exact text; every `export` line names
  `typesafe::API_KEY_ENV`.
- Each page names the *one* setting that gates its feature — `priority.md`
  must not mention `send_issue_text` — so a reader is never sent to enable
  the wrong flag.
- Every disclosure page states, in words, that the org (and for the two
  text-sending features, the repo) is never sent — the same claim
  ADR 0004 makes and #183 fixed a violation of.
- Every `HelpTopic` has exactly one page on disk, and no page on disk is
  orphaned.

Changing a threshold, a setting's name, or the indicator's wording without
updating the matching page fails CI rather than shipping a page that is
quietly wrong.

## Geometry

`ui::help_lines` builds every wrapped row a page occupies — the status block,
then the markdown (or the generated key table) — and both the renderer and
`ui::help_max_scroll` (the key handler's scroll clamp) call it, the same
pattern the PR summary popup uses. A clamp measured a different way could
drift from what is actually drawn and let scrolling run past the last row
into blank space; a test scrolls every page to its clamp at a small terminal
and asserts the page's own last line is the one on screen.

## Verification

- `cargo test`: all pages render without panicking at five terminal sizes,
  including 1×1 and a size that shows the whole page unscrolled.
- Sixteen hand mutations (return point, context table, F1 gating, the scroll
  clamp, the "any key closes" behaviour, status wording, the indicator's
  consent check, page/setting cross-checks) — each caught. Two survived
  first: the scroll-clamp test checked only that scrolling was *possible*,
  and the keys-page test checked for the string `"F1"` rather than the
  actual key-table row; both now assert the exact row count and the exact
  row respectively.
- Driven manually in a real terminal (`tmux`): `?` and `F1` from the list, a
  popup, and mid-typed search (buffer survives); every page in turn; the
  status block against a real config; scrolling to both ends; `F12 ?` inside
  a session; a clean quit. No panic in the session log.

## Not covered

- Help for the filter editor's other fields, sessions beyond `F12 ?`, move,
  PR summary or the new-issue form. They are either self-explanatory in
  their own popups or covered adequately by the keys page.
- Clickable links inside help pages — the pages avoid Markdown links for
  this reason (a link would show its label and go nowhere).
- A live end-to-end run of a real semantic search or readiness request from
  inside the help-driven flow — the request paths are `#158`/`#160`'s to
  verify; this ticket verified the pages and the viewer around them.
