//! Inline (character-level) markdown: bold, italic, code, links, escapes.
//!
//! [`parse_inline_links`] turns a run of text into styled [`Span`]s and reports
//! every URL's display-column range within that run, so the caller can offset
//! those columns past whatever prefix (list bullet, quote bar, table cell) the
//! run sits behind.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::{code_style, link_style};

/// A link located within a single parsed span run: display-column range plus URL.
pub(super) struct Local {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// Inline span parser: bold (`**`/`__`), italic (`*`/`_`), inline code
/// (`` ` ``), links (`[text](url)`), bare `http(s)://` URLs, and `\` escapes.
/// Also reports every link's display-column span so the label / URL text can be
/// made clickable. Columns are relative to the start of the returned span run
/// (the caller offsets past any list prefix).
pub(super) fn parse_inline_links(text: &str, t: &super::Theme) -> (Vec<Span<'static>>, Vec<Local>) {
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
