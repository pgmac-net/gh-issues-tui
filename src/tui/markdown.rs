//! Simple, line-oriented markdown renderer for the detail pane.
//!
//! `render` emits exactly one output [`Line`] per input line — block styling
//! (headings, fences, quotes, lists) never adds or drops a line, only restyles
//! it. The detail pane's scroll clamps measure *wrapped* height with ratatui's
//! `Paragraph::line_count`, so this one-line-per-source-line property is no
//! longer load-bearing for scroll sync, but it keeps the mapping predictable.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

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

/// Render `body` as styled markdown, one output line per input line, and report
/// every URL's position (bare `http(s)://` URLs and markdown `[label](url)`
/// links) so the caller can make them clickable. Fenced code and headings are
/// not scanned for links.
pub fn render_with_links(body: &str, t: &Theme) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
    let mut out = Vec::with_capacity(body.lines().count());
    let mut links = Vec::new();
    let mut in_fence = false;
    let mut fence_char = '`';

    for (idx, raw) in body.lines().enumerate() {
        let trimmed = raw.trim_start();

        if let Some(c) = fence_open_char(trimmed) {
            if in_fence && c == fence_char {
                in_fence = false;
            } else if !in_fence {
                in_fence = true;
                fence_char = c;
            }
            out.push(Line::styled(raw.to_string(), code_style(t)));
            continue;
        }

        if in_fence {
            out.push(Line::styled(raw.to_string(), code_style(t)));
            continue;
        }

        let (line, locals) = render_line_links(raw, trimmed, t);
        for l in locals {
            links.push(LinkSpan {
                line: idx,
                col_start: l.start,
                col_end: l.end,
                url: l.url,
            });
        }
        out.push(line);
    }

    (out, links)
}

/// A link located within a single parsed span run: display-column range plus URL.
struct Local {
    start: usize,
    end: usize,
    url: String,
}

fn fence_open_char(trimmed: &str) -> Option<char> {
    ['`', '~']
        .into_iter()
        .find(|&c| trimmed.chars().take_while(|&x| x == c).count() >= 3)
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
    Style::default().fg(t.dim)
}

fn link_style(t: &Theme) -> Style {
    Style::default()
        .fg(t.accent)
        .add_modifier(Modifier::UNDERLINED)
}

/// Inline span parser: bold (`**`/`__`), italic (`*`/`_`), inline code
/// (`` ` ``), links (`[text](url)`), bare `http(s)://` URLs, and `\` escapes.
/// Also reports every link's display-column span so the label / URL text can be
/// made clickable. Columns are relative to the start of the returned span run
/// (the caller offsets past any list prefix).
fn parse_inline_links(text: &str, t: &Theme) -> (Vec<Span<'static>>, Vec<Local>) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans = Vec::new();
    // (span index, url) for each clickable span; columns are resolved below.
    let mut marks: Vec<(usize, String)> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        match c {
            '\\' if i + 1 < n => {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                if let Some(close) = find_char(&chars, i + 1, '`') {
                    flush(&mut spans, &mut buf);
                    let inner: String = chars[i + 1..close].iter().collect();
                    spans.push(Span::styled(inner, code_style(t)));
                    i = close + 1;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            '*' | '_' => {
                let marker = c;
                let double = i + 1 < n && chars[i + 1] == marker;
                let marker_len = if double { 2 } else { 1 };
                let search_from = i + marker_len;
                match find_marker(&chars, search_from, marker, marker_len) {
                    Some(close) if close > search_from => {
                        flush(&mut spans, &mut buf);
                        let inner: String = chars[search_from..close].iter().collect();
                        let style = if double {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::ITALIC)
                        };
                        spans.push(Span::styled(inner, style));
                        i = close + marker_len;
                    }
                    _ => {
                        buf.push(c);
                        i += 1;
                    }
                }
            }
            '[' => {
                if let Some((close_bracket, close_paren)) = find_link(&chars, i) {
                    flush(&mut spans, &mut buf);
                    let label: String = chars[i + 1..close_bracket].iter().collect();
                    let url: String = chars[close_bracket + 2..close_paren].iter().collect();
                    if !url.is_empty() {
                        marks.push((spans.len(), url));
                    }
                    spans.push(Span::styled(label, link_style(t)));
                    i = close_paren + 1;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            _ => {
                if let Some(end) = bare_url_end(&chars, i) {
                    flush(&mut spans, &mut buf);
                    let url: String = chars[i..end].iter().collect();
                    marks.push((spans.len(), url.clone()));
                    spans.push(Span::styled(url, link_style(t)));
                    i = end;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
        }
    }
    flush(&mut spans, &mut buf);

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    // Resolve each mark's column span from the cumulative width of prior spans.
    let mut starts = Vec::with_capacity(spans.len() + 1);
    let mut acc = 0usize;
    for s in &spans {
        starts.push(acc);
        acc += UnicodeWidthStr::width(s.content.as_ref());
    }
    starts.push(acc);
    let locals = marks
        .into_iter()
        .map(|(idx, url)| Local {
            start: starts[idx],
            end: starts[idx + 1],
            url,
        })
        .collect();

    (spans, locals)
}

/// If a bare `http(s)://` URL starts at `chars[i]`, return its end index
/// (exclusive). The URL must sit on a boundary (not mid-word), extends to the
/// next whitespace or URL-hostile character, and has trailing sentence
/// punctuation trimmed (a closing `)` is kept only when the URL opened one).
fn bare_url_end(chars: &[char], i: usize) -> Option<usize> {
    if i > 0 && chars[i - 1].is_alphanumeric() {
        return None;
    }
    let scheme_len = if chars[i..].starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/']) {
        8
    } else if chars[i..].starts_with(&['h', 't', 't', 'p', ':', '/', '/']) {
        7
    } else {
        return None;
    };

    let body_start = i + scheme_len;
    let mut end = body_start;
    while end < chars.len() {
        let c = chars[end];
        if c.is_whitespace()
            || matches!(
                c,
                '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^' | '[' | ']'
            )
        {
            break;
        }
        end += 1;
    }
    if end == body_start {
        return None; // scheme with no host
    }

    while end > body_start {
        let c = chars[end - 1];
        let trim = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            // Trim a trailing `)` only when it is unbalanced (more closes than
            // opens in the URL so far), so `Foo_(bar)` keeps its own paren but a
            // wrapping `(url)` does not.
            ')' => {
                let opens = chars[i..end].iter().filter(|&&x| x == '(').count();
                let closes = chars[i..end].iter().filter(|&&x| x == ')').count();
                closes > opens
            }
            _ => false,
        };
        if trim {
            end -= 1;
        } else {
            break;
        }
    }

    Some(end)
}

fn flush(spans: &mut Vec<Span<'static>>, buf: &mut String) {
    if !buf.is_empty() {
        spans.push(Span::raw(std::mem::take(buf)));
    }
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    chars[from..]
        .iter()
        .position(|&c| c == target)
        .map(|p| p + from)
}

fn find_marker(chars: &[char], from: usize, marker: char, len: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = from;
    while i + len <= n {
        if chars[i..i + len].iter().all(|&c| c == marker) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_link(chars: &[char], start: usize) -> Option<(usize, usize)> {
    let n = chars.len();
    let close_bracket = find_char(chars, start + 1, ']')?;
    if close_bracket + 1 >= n || chars[close_bracket + 1] != '(' {
        return None;
    }
    let close_paren = find_char(chars, close_bracket + 2, ')')?;
    Some((close_bracket, close_paren))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Styled lines only — the link positions are exercised separately.
    fn render(body: &str, t: &Theme) -> Vec<Line<'static>> {
        render_with_links(body, t).0
    }

    fn spans_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn line_count_matches_source_for_mixed_body() {
        let body = "# Title\n\nSome *text* and __bold__.\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n> quoted\n\n1. first\n2. second\n";
        let t = Theme::default();
        assert_eq!(render(body, &t).len(), body.lines().count());
    }

    #[test]
    fn line_count_matches_for_unterminated_fence() {
        let body = "```\nfn x() {}\nstill in fence\n";
        let t = Theme::default();
        assert_eq!(render(body, &t).len(), body.lines().count());
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
    fn inline_code_span_is_dim_styled() {
        let t = Theme::default();
        let lines = render("run `cargo test` now", &t);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "cargo test")
            .expect("code span present");
        assert_eq!(code_span.style.fg, Some(t.dim));
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
        assert_eq!(spans_text(&lines[1]), "**not bold**");
        assert_eq!(lines[1].style.fg, Some(t.dim));
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
        let (lines, links) = render_with_links("see https://example.com/x now", &t);
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
        let (_, links) = render_with_links("see [the docs](https://example.com/x) here", &t);
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
        let (_, links) = render_with_links("visit https://example.com/path.", &t);
        assert_eq!(links[0].url, "https://example.com/path");
        // A closing paren is kept when the URL opened one.
        let (_, balanced) = render_with_links("(https://en.wikipedia.org/wiki/Foo_(bar))", &t);
        assert_eq!(balanced[0].url, "https://en.wikipedia.org/wiki/Foo_(bar)");
    }

    #[test]
    fn url_inside_inline_code_is_not_linked() {
        let t = Theme::default();
        let (_, links) = render_with_links("run `curl https://example.com`", &t);
        assert!(links.is_empty());
    }

    #[test]
    fn url_inside_fence_is_not_linked() {
        let t = Theme::default();
        let (_, links) = render_with_links("```\nhttps://example.com\n```", &t);
        assert!(links.is_empty());
    }

    #[test]
    fn link_columns_offset_past_list_prefix() {
        let t = Theme::default();
        // "• " prefix is 2 cols, so a URL at the start of the item begins at col 2.
        let (_, links) = render_with_links("- https://example.com", &t);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].col_start, 2);
    }

    #[test]
    fn url_midword_is_not_detected() {
        let t = Theme::default();
        let (_, links) = render_with_links("xhttps://example.com", &t);
        assert!(links.is_empty());
    }
}
