//! Simple, line-oriented markdown renderer for the detail pane.
//!
//! Most blocks emit exactly one output [`Line`] per input line — headings,
//! quotes and lists restyle a line, they never add or drop one. [`table`]
//! (#99) and [`fence`] (#120) are the deliberate exceptions: a table consumes
//! a whole source block and wraps cells inside their columns, and a fence
//! drops both delimiter lines and hard-breaks each content line at the pane
//! edge, so either can turn one source row into a different number of screen
//! rows.
//!
//! That is safe because the detail pane's scroll clamps measure *wrapped*
//! height by rendering through this same function, so measured and drawn
//! heights cannot disagree. It does mean [`LinkSpan::line`] must be indexed
//! against the output line, never the source line.

mod fence;
mod highlight;
mod inline;
mod table;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;
use inline::{Local, parse_inline_links};

/// A URL and the display-column span it occupies within a rendered line.
///
/// `line` indexes the `Vec<Line>` returned alongside these spans; `col_start..col_end`
/// are display columns (terminal cells) from the start of that rendered line. The
/// hyperlink layer ([`super::linkmap`]) maps these onto wrapped, scrolled cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub url: String,
}

/// Render `body` as styled markdown into a pane `width` cells wide, and report
/// every URL's position (bare `http(s)://` URLs and markdown `[label](url)`
/// links) so the caller can make them clickable. Fenced code and headings are
/// not scanned for links.
///
/// `width` is only consulted for tables, which lay their columns out to fit;
/// every other block is emitted unwrapped for [`super::linkmap`] to wrap.
pub fn render_with_links(
    body: &str,
    width: usize,
    t: &Theme,
) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
    let src: Vec<&str> = body.lines().collect();
    let mut out = Vec::with_capacity(src.len());
    let mut links = Vec::new();

    let mut i = 0;
    while i < src.len() {
        let raw = src[i];
        let trimmed = raw.trim_start();

        // Fences and tables consume several source lines at once and emit a
        // different number of output lines, so both are dispatched before the
        // per-line block rules. Fence first: a `|` inside code must never be
        // mistaken for a table.
        if let Some((fence, used)) = fence::parse(&src[i..]) {
            out.extend(fence::render(&fence, width, t));
            i += used;
            continue;
        }

        if let Some((table, used)) = table::parse(&src[i..]) {
            let (rows, row_links) = table.render(width, t);
            let base = out.len();
            links.extend(row_links.into_iter().map(|mut l| {
                l.line += base;
                l
            }));
            out.extend(rows);
            i += used;
            continue;
        }

        let (line, locals) = render_line_links(raw, trimmed, t);
        // Indexed against the *output* line, which no longer tracks the source
        // line index once a table has expanded rows above it.
        let line_idx = out.len();
        for l in locals {
            links.push(LinkSpan {
                line: line_idx,
                col_start: l.start,
                col_end: l.end,
                url: l.url,
            });
        }
        out.push(line);
        i += 1;
    }

    (out, links)
}

/// Render one non-fenced source line, returning the styled line plus any links
/// found in its inline content (columns already offset past list/quote prefixes).
/// Headings are not inline-parsed today, so they carry no links.
fn render_line_links(raw: &str, trimmed: &str, t: &Theme) -> (Line<'static>, Vec<Local>) {
    if trimmed.is_empty() {
        return (Line::default(), Vec::new());
    }

    if let Some(rest) = heading_rest(trimmed) {
        let style = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        return (Line::styled(rest.to_string(), style), Vec::new());
    }

    if is_hr(trimmed) {
        return (
            Line::styled("─".repeat(40), Style::default().fg(t.dim)),
            Vec::new(),
        );
    }

    if let Some(rest) = trimmed
        .strip_prefix("> ")
        .or_else(|| trimmed.strip_prefix('>'))
    {
        let prefix = Span::styled("▏ ", Style::default().fg(t.dim));
        return prefixed_line(&[prefix], rest.trim_start(), t);
    }

    let indent = &raw[..raw.len() - trimmed.len()];

    if let Some(rest) = unordered_rest(trimmed) {
        let prefix = [
            Span::raw(indent.to_string()),
            Span::styled("• ", Style::default().fg(t.accent)),
        ];
        return prefixed_line(&prefix, rest, t);
    }

    if let Some((marker, rest)) = ordered_rest(trimmed) {
        let prefix = [
            Span::raw(indent.to_string()),
            Span::styled(format!("{marker} "), Style::default().fg(t.accent)),
        ];
        return prefixed_line(&prefix, rest, t);
    }

    let (spans, locals) = parse_inline_links(raw, t);
    (Line::from(spans), locals)
}

/// Build a line from fixed `prefix` spans followed by inline-parsed `text`,
/// shifting each link's columns past the prefix width.
fn prefixed_line(prefix: &[Span<'static>], text: &str, t: &Theme) -> (Line<'static>, Vec<Local>) {
    let offset: usize = prefix
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let (inline, mut locals) = parse_inline_links(text, t);
    for l in &mut locals {
        l.start += offset;
        l.end += offset;
    }
    let mut spans: Vec<Span<'static>> = prefix.to_vec();
    spans.extend(inline);
    (Line::from(spans), locals)
}

fn heading_rest(trimmed: &str) -> Option<&str> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if rest.is_empty() {
        return Some(rest);
    }
    rest.strip_prefix(' ')
}

fn is_hr(trimmed: &str) -> bool {
    for c in ['-', '*', '_'] {
        let stripped: String = trimmed.chars().filter(|&x| x != ' ').collect();
        if stripped.len() >= 3 && stripped.chars().all(|x| x == c) {
            return true;
        }
    }
    false
}

fn unordered_rest(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

fn ordered_rest(trimmed: &str) -> Option<(&str, &str)> {
    let digits_end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let sep = *trimmed.as_bytes().get(digits_end)?;
    if sep != b'.' && sep != b')' {
        return None;
    }
    let rest = trimmed[digits_end + 1..].strip_prefix(' ')?;
    Some((&trimmed[..=digits_end], rest))
}

fn code_style(t: &Theme) -> Style {
    Style::default().fg(t.code_fg).bg(t.code_bg)
}

/// Code style for one scanned token (#122). Every kind keeps `code_bg`, so a
/// highlighted row fills exactly as a flat one does; only the foreground
/// moves. [`highlight::TokenKind::Text`] is `code_fg`, i.e. pre-#122
/// rendering.
fn token_style(t: &Theme, kind: highlight::TokenKind) -> Style {
    let fg = match kind {
        highlight::TokenKind::Keyword => t.code_keyword,
        highlight::TokenKind::Str => t.code_string,
        highlight::TokenKind::Comment => t.code_comment,
        highlight::TokenKind::Number => t.code_number,
        highlight::TokenKind::Text => t.code_fg,
    };
    Style::default().fg(fg).bg(t.code_bg)
}

fn link_style(t: &Theme) -> Style {
    Style::default()
        .fg(t.accent)
        .add_modifier(Modifier::UNDERLINED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Styled lines only — the link positions are exercised separately.
    fn render(body: &str, t: &Theme) -> Vec<Line<'static>> {
        render_with_links(body, 80, t).0
    }

    fn spans_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn line_count_matches_source_for_mixed_body_minus_fence_delimiters() {
        let body = "# Title\n\nSome *text* and __bold__.\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n> quoted\n\n1. first\n2. second\n";
        let t = Theme::default();
        // Every block except the fence stays one line in, one line out; the
        // fence's two delimiter lines are dropped entirely (#120).
        assert_eq!(render(body, &t).len(), body.lines().count() - 2);
    }

    #[test]
    fn line_count_matches_for_unterminated_fence_minus_opening_delimiter() {
        let body = "```\nfn x() {}\nstill in fence\n";
        let t = Theme::default();
        assert_eq!(render(body, &t).len(), body.lines().count() - 1);
    }

    #[test]
    fn empty_line_yields_default() {
        let t = Theme::default();
        let lines = render("\n", &t);
        assert_eq!(lines.len(), 1);
        assert_eq!(spans_text(&lines[0]), "");
    }

    #[test]
    fn heading_is_bold_accent_and_strips_hashes() {
        let t = Theme::default();
        let lines = render("## Heading text", &t);
        assert_eq!(spans_text(&lines[0]), "Heading text");
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[0].style.fg, Some(t.accent));
    }

    #[test]
    fn bold_and_italic_inline_spans() {
        let t = Theme::default();
        let lines = render("a **bold** b *em* c __also bold__ d _also em_", &t);
        let styles: Vec<Modifier> = lines[0]
            .spans
            .iter()
            .map(|s| s.style.add_modifier)
            .collect();
        assert!(styles.contains(&Modifier::BOLD));
        assert!(styles.contains(&Modifier::ITALIC));
        assert_eq!(spans_text(&lines[0]), "a bold b em c also bold d also em");
    }

    #[test]
    fn inline_code_span_is_code_styled() {
        let t = Theme::default();
        let lines = render("run `cargo test` now", &t);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "cargo test")
            .expect("code span present");
        assert_eq!(code_span.style.fg, Some(t.code_fg));
        assert_eq!(code_span.style.bg, Some(t.code_bg));
    }

    #[test]
    fn link_keeps_text_drops_url() {
        let t = Theme::default();
        let lines = render("see [the docs](https://example.com/x) here", &t);
        assert_eq!(spans_text(&lines[0]), "see the docs here");
        let link_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "the docs")
            .unwrap();
        assert!(link_span.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn escaped_asterisk_is_literal() {
        let t = Theme::default();
        let lines = render(r"\*not bold\*", &t);
        assert_eq!(spans_text(&lines[0]), "*not bold*");
        assert_eq!(lines[0].spans.len(), 1);
    }

    #[test]
    fn blockquote_gets_prefix_and_inline_parsing() {
        let t = Theme::default();
        let lines = render("> a **quoted** line", &t);
        assert!(spans_text(&lines[0]).starts_with("▏ a quoted line"));
    }

    #[test]
    fn unordered_list_bullet_replaces_marker() {
        let t = Theme::default();
        let lines = render("- item one", &t);
        assert_eq!(spans_text(&lines[0]), "• item one");
    }

    #[test]
    fn ordered_list_keeps_number() {
        let t = Theme::default();
        let lines = render("2. second item", &t);
        assert_eq!(spans_text(&lines[0]), "2. second item");
    }

    #[test]
    fn horizontal_rule_renders_dim_line() {
        let t = Theme::default();
        let lines = render("---", &t);
        assert_eq!(lines[0].style.fg, Some(t.dim));
    }

    #[test]
    fn fenced_code_is_not_inline_parsed() {
        let t = Theme::default();
        let body = "```\n**not bold**\n```";
        let lines = render(body, &t);
        // Fence delimiters are dropped (#120): the sole content line is lines[0].
        assert_eq!(lines.len(), 1);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "**not bold**")
            .expect("literal, un-parsed code text present");
        assert_eq!(code_span.style.fg, Some(t.code_fg));
        assert_eq!(code_span.style.bg, Some(t.code_bg));
    }

    #[test]
    fn plain_paragraph_is_single_raw_span() {
        let t = Theme::default();
        let lines = render("just a plain line", &t);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(spans_text(&lines[0]), "just a plain line");
    }

    #[test]
    fn bare_url_is_detected_with_columns() {
        let t = Theme::default();
        let (lines, links) = render_with_links("see https://example.com/x now", 80, &t);
        assert_eq!(spans_text(&lines[0]), "see https://example.com/x now");
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.url, "https://example.com/x");
        assert_eq!(l.line, 0);
        // "see " is 4 cols; the URL is 21 cols wide.
        assert_eq!(l.col_start, 4);
        assert_eq!(l.col_end, 25);
        // The URL span carries the link style.
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "https://example.com/x")
            .unwrap();
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn markdown_link_reports_url_over_label_columns() {
        let t = Theme::default();
        let (_, links) = render_with_links("see [the docs](https://example.com/x) here", 80, &t);
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.url, "https://example.com/x");
        // Label "the docs" sits at columns 4..12 (after "see ").
        assert_eq!(l.col_start, 4);
        assert_eq!(l.col_end, 12);
    }

    #[test]
    fn bare_url_trailing_punctuation_is_trimmed() {
        let t = Theme::default();
        let (_, links) = render_with_links("visit https://example.com/path.", 80, &t);
        assert_eq!(links[0].url, "https://example.com/path");
        // A closing paren is kept when the URL opened one.
        let (_, balanced) = render_with_links("(https://en.wikipedia.org/wiki/Foo_(bar))", 80, &t);
        assert_eq!(balanced[0].url, "https://en.wikipedia.org/wiki/Foo_(bar)");
    }

    #[test]
    fn url_inside_inline_code_is_not_linked() {
        let t = Theme::default();
        let (_, links) = render_with_links("run `curl https://example.com`", 80, &t);
        assert!(links.is_empty());
    }

    #[test]
    fn url_inside_fence_is_not_linked() {
        let t = Theme::default();
        let (_, links) = render_with_links("```\nhttps://example.com\n```", 80, &t);
        assert!(links.is_empty());
    }

    #[test]
    fn link_columns_offset_past_list_prefix() {
        let t = Theme::default();
        // "• " prefix is 2 cols, so a URL at the start of the item begins at col 2.
        let (_, links) = render_with_links("- https://example.com", 80, &t);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].col_start, 2);
    }

    #[test]
    fn url_midword_is_not_detected() {
        let t = Theme::default();
        let (_, links) = render_with_links("xhttps://example.com", 80, &t);
        assert!(links.is_empty());
    }
}
