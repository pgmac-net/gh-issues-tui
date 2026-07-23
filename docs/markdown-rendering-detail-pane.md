# Markdown rendering in the detail pane (#67, 2026-07-23)

Ticket: pgmac-net/gh-issues-tui#67 · PR: #71

## What changed

Issue descriptions and comment bodies in the detail pane now render as
lightweight markdown instead of raw text: `#` headings, `**bold**`/`*italic*`,
`` `inline code` ``, fenced ` ``` ` code blocks, `> ` blockquotes, `-`/`*`/`+`
and numbered lists, `---` horizontal rules, and `[text](url)` links (shown as
styled text — the URL is dropped, keeping the pane readable in a terminal).

## Approach

A small in-house renderer (`src/tui/markdown.rs`), not a dependency. Two
disqualified `tui-markdown`: it reflows paragraphs and lags the pinned
ratatui 0.30. The project's existing ethos (rustls over a keyring, OSC 52
over a clipboard crate) also favours a minimal, self-contained solution here.

**Property:** `render()` emits exactly one output `Line` per source line —
block styling (headings, fences, quotes, lists) restyles a line, it never adds
or removes one. This originally kept a source-line-counting scroll cursor in
sync with what `ui::draw_detail` paints. Since the #59 detail-pane redesign the
scroll clamps measure *wrapped* height with ratatui's `Paragraph::line_count`
(`unstable-rendered-line-info` feature) instead, so this one-line-per-source
property is no longer load-bearing for scroll sync — but it keeps the source→
screen mapping predictable and is cheap to preserve.

Block dispatch happens per line (only fenced-code state carries across
lines); an inline character-scanner then produces styled `Span`s for bold,
italic, inline code, and links within non-fence lines. Heading/HR lines use
`Line::styled` (style carried at the `Line` level, patched onto spans at
render time by ratatui — confirmed via `ratatui-core`'s `Line::styled`
source, which doesn't push it onto the span itself).

Colours reuse the existing `Theme` (`accent`, `dim`) — no schema change to
`theme.rs` or the config file.

## Deviations from the plan

None. The plan's line-count invariant, module boundary, and out-of-scope
list (tables, nested-list re-indent, code-fence syntax highlighting) all
held through implementation.

One correction during implementation: unit tests initially asserted style
on `Span`, which failed — `Line::styled` sets `Line.style`, not per-span
style (spans stay `Span::raw`). Fixed by asserting on `Line.style` instead;
no renderer logic changed, only what the tests inspected.

## Testing

- 14 new unit tests in `tui::markdown` (line-count invariant across mixed
  bodies and an unterminated fence, each inline style, escapes, block types).
  Full suite: 254 passed.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.
- Live-verified against real GitHub data (`pgmac-net/homelabia#149`, a body
  with bold text, inline code, and a bullet list) using the project's
  pty + pyte screen-capture recipe (`.claude/skills/verify/SKILL.md`):
  bullets rendered as `•`, `**bold**` markers stripped, inline-code
  backticks stripped, and detail-pane navigation/scrolling were unaffected.
