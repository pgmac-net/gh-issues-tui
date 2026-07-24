# Clickable URLs in the detail pane (#80, 2026-07-24)

Ticket: pgmac-net/gh-issues-tui#80

## What changed

URLs in an issue's description and in its comments are now **clickable**. Both
bare `http(s)://` URLs written in the text and the labels of markdown
`[text](url)` links open in the operating system's default browser. This
supersedes the previous behaviour where a markdown link's URL was dropped (see
[`markdown-rendering-detail-pane.md`](markdown-rendering-detail-pane.md)).

Activation is **Ctrl+Click** (**Cmd+Click** on macOS) — the standard terminal
hyperlink gesture.

## Approach — terminal-native OSC 8 hyperlinks

Rather than have the application capture the mouse and open URLs itself, the
detail pane emits **OSC 8 hyperlink escape sequences** around each URL's cells.
The terminal itself makes them clickable and hands the URL to the OS default
browser. This was chosen over app-side mouse capture because:

- **No mouse capture** → the terminal's native text selection / copy is
  unaffected (mouse capture would have forced a Shift+drag workaround).
- **Cross-platform for free** — the terminal, not the app, resolves the browser,
  on Linux, macOS and Windows alike.
- Terminals without OSC 8 support **degrade gracefully**: the URL is simply shown
  as plain underlined text.

Keyboard navigation of links was explicitly out of scope for this ticket.

### Pipeline

1. **`src/tui/markdown.rs` — `render_with_links`.** Alongside the styled lines,
   the renderer now reports a `LinkSpan { line, col_start, col_end, url }` for
   every URL: bare URLs detected by an in-text scanner (boundary-anchored,
   trailing sentence punctuation trimmed, balanced-paren aware), and
   `[label](url)` links whose *label* becomes the hotspot. Headings and
   fenced/inline code are not scanned.

2. **`src/tui/linkmap.rs` (new) — `wrap`.** The detail regions now **own their
   word-wrapping** instead of leaning on `Paragraph`'s internal wrap. `wrap`
   pre-wraps the lines (rendered by a `Paragraph` with wrapping *off*) and, in
   the same pass, maps each `LinkSpan` onto the exact wrapped, scrolled cells it
   occupies, returning a `LinkRect` per URL run. A URL crossing a wrap boundary
   yields several rects that share one `id` so the terminal treats them as one
   link. `wrapped_height` counts rows the same way, so scroll-clamp measurement
   and rendering can't drift. (This removed the earlier
   `Paragraph::line_count` / `unstable-rendered-line-info` approach.)

3. **`src/tui/ui.rs` — `apply_hyperlinks`.** After each region's `Paragraph`
   draws, this walks the known `LinkRect`s and rewrites the buffer cells: it
   prepends `\e]8;id=N;URL\e\\` to a run's first visible cell and appends
   `\e]8;;\e\\` to its last, each pinned with `CellDiffOption::ForcedWidth` so
   the escape bytes don't disturb ratatui's layout or frame diffing. Scroll and
   the viewport clip rects to what's on screen.

`body_lines_links` / `comment_card_lines_links` produce the region lines plus
their `LinkSpan`s with line indices offset past the metadata header / comment
header rows.

## Files

| File | Change |
|------|--------|
| `Cargo.toml` | Add `unicode-segmentation`, `unicode-width`; drop the now-unused ratatui `unstable-rendered-line-info` feature. |
| `src/tui/markdown.rs` | `LinkSpan`, `render_with_links`, bare-URL scanner; md-link URLs captured instead of dropped. |
| `src/tui/linkmap.rs` | New module: owned word-wrap + link-to-cell mapping + `wrapped_height`. |
| `src/tui/mod.rs` | Register `pub mod linkmap`. |
| `src/tui/ui.rs` | Detail regions pre-wrap via `linkmap`; `apply_hyperlinks` OSC 8 buffer surgery; `*_links` line builders; `paragraph_height` delegates to `linkmap`. |
| `README.md`, `CLAUDE.md` | Document clickable URLs and the owned-wrap change. |

## Testing

- `markdown`: bare-URL detection with columns, trailing-punctuation and
  balanced-paren trimming, mid-word rejection, inline-code / fenced-code
  exclusion, markdown-link target capture, list-prefix column offset.
- `linkmap`: wrap row counts, break-after-whitespace, hard-break of long tokens,
  `wrapped_height` == `wrap` row count, single- and multi-row `LinkRect`
  mapping (shared `id` across a wrapped link).
- `ui`: a `TestBackend` render of the detail pane asserts the OSC 8 open/close
  sequences bracket both a bare body URL and a comment link's target, and that
  the visible URL text is preserved.

Full suite: `cargo test` (297 tests), `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check`.

## Not covered (v1)

- List-view row URLs and the PR-summary popup (both already have `o` / `Enter`
  open actions).
- Keyboard-driven link navigation.
- Plain single-click activation (OSC 8 uses the terminal's modifier-click).
