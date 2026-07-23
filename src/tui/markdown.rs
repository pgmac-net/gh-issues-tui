//! Simple, line-oriented markdown renderer for the detail pane.
//!
//! `App::comment_card_lines` / `App::detail_card_offset` count *source*
//! lines to keep the comment-card scroll cursor in sync with what
//! `ui::draw_detail` paints, so `render` must emit exactly one output
//! [`Line`] per input line — block styling (headings, fences, quotes,
//! lists) never adds or drops a line, only restyles it.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme::Theme;

/// Render `body` as styled markdown, one output line per input line.
pub fn render(body: &str, t: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(body.lines().count());
    let mut in_fence = false;
    let mut fence_char = '`';

    for raw in body.lines() {
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

        out.push(render_line(raw, trimmed, t));
    }

    out
}

fn fence_open_char(trimmed: &str) -> Option<char> {
    ['`', '~']
        .into_iter()
        .find(|&c| trimmed.chars().take_while(|&x| x == c).count() >= 3)
}

fn render_line(raw: &str, trimmed: &str, t: &Theme) -> Line<'static> {
    if trimmed.is_empty() {
        return Line::default();
    }

    if let Some(rest) = heading_rest(trimmed) {
        let style = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        return Line::styled(rest.to_string(), style);
    }

    if is_hr(trimmed) {
        return Line::styled("─".repeat(40), Style::default().fg(t.dim));
    }

    if let Some(rest) = trimmed
        .strip_prefix("> ")
        .or_else(|| trimmed.strip_prefix('>'))
    {
        let mut spans = vec![Span::styled("▏ ", Style::default().fg(t.dim))];
        spans.extend(parse_inline(rest.trim_start(), t));
        return Line::from(spans);
    }

    let indent = &raw[..raw.len() - trimmed.len()];

    if let Some(rest) = unordered_rest(trimmed) {
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled("• ", Style::default().fg(t.accent)),
        ];
        spans.extend(parse_inline(rest, t));
        return Line::from(spans);
    }

    if let Some((marker, rest)) = ordered_rest(trimmed) {
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(format!("{marker} "), Style::default().fg(t.accent)),
        ];
        spans.extend(parse_inline(rest, t));
        return Line::from(spans);
    }

    Line::from(parse_inline(raw, t))
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
/// (`` ` ``), links (`[text](url)`, url dropped), and `\` escapes.
fn parse_inline(text: &str, t: &Theme) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans = Vec::new();
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
                    spans.push(Span::styled(label, link_style(t)));
                    i = close_paren + 1;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush(&mut spans, &mut buf);

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
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
}
