//! Fenced code blocks (``` ``` ``` or `~~~`).
//!
//! Unlike the rest of [`super`], a fence is *not* one output [`Line`] per
//! source line: both fence delimiter lines are dropped, and each content line
//! hard-breaks at the pane edge rather than word-wrapping, so one source line
//! can still become several rows in a narrow pane. The block is pre-wrapped
//! here (mirroring [`super::table`]) precisely so [`super::super::linkmap`]
//! never re-wraps it — that's what keeps the gutter bar and background fill
//! attached to every continuation row.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::Theme;

/// Gutter prefix on every code row: a dim bar plus one padding cell, echoing
/// the blockquote's `▏ ` prefix in `mod.rs`.
const GUTTER: &str = "▏ ";
const GUTTER_WIDTH: usize = 2;

/// A parsed fence: its optional language tag (the fence's info string) and
/// literal content lines, neither fence delimiter included.
pub(super) struct Fence {
    lang: Option<String>,
    content: Vec<String>,
}

/// Parse a fence starting at `src[0]`, returning it with the number of source
/// lines consumed (including the closing delimiter, when present). `None`
/// when `src` does not start a fence. An unterminated fence consumes every
/// remaining line.
pub(super) fn parse(src: &[&str]) -> Option<(Fence, usize)> {
    let first = *src.first()?;
    let trimmed = first.trim_start();
    let (fence_char, fence_len) = fence_open(trimmed)?;
    let info = trimmed[fence_len..].trim();
    let lang = if info.is_empty() {
        None
    } else {
        Some(info.to_string())
    };

    let mut content = Vec::new();
    let mut used = 1;
    while let Some(&line) = src.get(used) {
        if is_closing(line.trim_start(), fence_char) {
            used += 1;
            return Some((Fence { lang, content }, used));
        }
        content.push(line.to_string());
        used += 1;
    }
    Some((Fence { lang, content }, used))
}

/// If `trimmed` opens a fence, return its char (`` ` `` or `~`) and run length.
fn fence_open(trimmed: &str) -> Option<(char, usize)> {
    ['`', '~'].into_iter().find_map(|c| {
        let n = trimmed.chars().take_while(|&x| x == c).count();
        (n >= 3).then_some((c, n))
    })
}

fn is_closing(trimmed: &str, fence_char: char) -> bool {
    fence_open(trimmed).is_some_and(|(c, _)| c == fence_char)
}

/// Render a fence's content as gutter-prefixed rows on a filled code
/// background, hard-breaking each content line at `width` cells (no
/// whitespace-seeking — a code line is never re-flowed). The language tag, if
/// present, is drawn right-aligned on the block's very first row only, and
/// only when it fits alongside that row's content; otherwise it is dropped
/// rather than truncating code.
pub(super) fn render(fence: &Fence, width: usize, t: &Theme) -> Vec<Line<'static>> {
    if fence.content.is_empty() {
        return Vec::new();
    }

    let code_style = super::code_style(t);

    // Degenerate pane: no room for a gutter. Fall back to flat, unpadded rows
    // so we never panic on an empty or negative code area.
    if width <= GUTTER_WIDTH {
        return fence
            .content
            .iter()
            .map(|l| Line::styled(l.clone(), code_style))
            .collect();
    }

    let code_area = width - GUTTER_WIDTH;
    let gutter_style = Style::default().fg(t.dim).bg(t.code_bg);
    let tag_style = gutter_style.add_modifier(Modifier::ITALIC);
    let tag = fence.lang.as_ref().map(|l| format!(" {l}"));

    let mut out = Vec::new();
    for line in &fence.content {
        for chunk in hard_break(line, code_area) {
            let chunk_w = UnicodeWidthStr::width(chunk.as_str());
            let mut spans = vec![Span::styled(GUTTER, gutter_style)];

            let tag_drawn = out.is_empty()
                && tag.as_ref().is_some_and(|tag| {
                    let tag_w = UnicodeWidthStr::width(tag.as_str());
                    code_area.saturating_sub(chunk_w) >= tag_w
                });

            spans.push(Span::styled(chunk, code_style));
            let pad = code_area.saturating_sub(chunk_w);
            if tag_drawn {
                let tag = tag.as_ref().expect("tag_drawn implies tag is Some");
                let tag_w = UnicodeWidthStr::width(tag.as_str());
                let lead = pad - tag_w;
                if lead > 0 {
                    spans.push(Span::styled(" ".repeat(lead), code_style));
                }
                spans.push(Span::styled(tag.clone(), tag_style));
            } else if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), code_style));
            }

            out.push(Line::from(spans));
        }
    }
    out
}

/// Split `line` into chunks of at most `width` display cells, breaking
/// strictly at the width boundary (never at whitespace). A single grapheme
/// wider than `width` is emitted alone so progress is always made. An empty
/// line yields one empty chunk.
fn hard_break(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for g in line.graphemes(true) {
        let gw = UnicodeWidthStr::width(g);
        if cur_w + gw > width && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push_str(g);
        cur_w += gw;
    }
    rows.push(cur);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn parses_terminated_fence_with_lang() {
        let src = ["```rust", "fn x() {}", "```", "after"];
        let (fence, used) = parse(&src).unwrap();
        assert_eq!(used, 3);
        assert_eq!(fence.lang.as_deref(), Some("rust"));
        assert_eq!(fence.content, vec!["fn x() {}".to_string()]);
    }

    #[test]
    fn parses_unterminated_fence_to_eof() {
        let src = ["```", "fn x() {}", "still in fence"];
        let (fence, used) = parse(&src).unwrap();
        assert_eq!(used, 3);
        assert_eq!(fence.lang, None);
        assert_eq!(fence.content.len(), 2);
    }

    #[test]
    fn tilde_fence_parses() {
        let src = ["~~~", "code", "~~~"];
        let (fence, used) = parse(&src).unwrap();
        assert_eq!(used, 3);
        assert_eq!(fence.content, vec!["code".to_string()]);
    }

    #[test]
    fn non_fence_line_returns_none() {
        assert!(parse(&["plain text"]).is_none());
    }

    #[test]
    fn delimiter_lines_are_dropped_from_output() {
        let t = Theme::default();
        let src = ["```rust", "fn x() {}", "```"];
        let (fence, _) = parse(&src).unwrap();
        let lines = render(&fence, 80, &t);
        // Only the one content line remains; no fence markers survive.
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).contains("fn x() {}"));
    }

    #[test]
    fn empty_fence_renders_no_rows() {
        let t = Theme::default();
        let (fence, _) = parse(&["```", "```"]).unwrap();
        assert!(render(&fence, 80, &t).is_empty());
    }

    #[test]
    fn every_row_is_exactly_width_cells() {
        let t = Theme::default();
        let (fence, _) =
            parse(&["```", "short", "a much longer line of code here", "```"]).unwrap();
        for width in [10usize, 20, 40, 80] {
            let lines = render(&fence, width, &t);
            for l in &lines {
                let w: usize = l
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                assert_eq!(w, width, "row width mismatch at pane width {width}");
            }
        }
    }

    #[test]
    fn hard_break_loses_no_character_and_fits_width() {
        let long = "x".repeat(55);
        let (fence, _) = parse(&["```", &long, "```"]).unwrap();
        let t = Theme::default();
        let lines = render(&fence, 22, &t); // code_area = 20
        let rebuilt: String = lines
            .iter()
            .map(|l| {
                // Strip gutter (first span) and trailing padding to recover code text.
                l.spans
                    .get(1)
                    .map(|s| s.content.as_ref().trim_end())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(rebuilt, long);
    }

    #[test]
    fn lang_tag_drawn_right_aligned_when_it_fits() {
        let t = Theme::default();
        let (fence, _) = parse(&["```rust", "x", "```"]).unwrap();
        let lines = render(&fence, 20, &t); // code_area = 18, plenty of room
        let text = line_text(&lines[0]);
        assert!(text.trim_end().ends_with("rust"));
    }

    #[test]
    fn lang_tag_dropped_when_first_row_is_full() {
        let t = Theme::default();
        let long_first_line = "x".repeat(30);
        let (fence, _) = parse(&["```rust", &long_first_line, "```"]).unwrap();
        let lines = render(&fence, 20, &t); // code_area = 18, first row fully occupied by code
        let text = line_text(&lines[0]);
        assert!(!text.contains("rust"));
    }

    #[test]
    fn bare_fence_draws_no_tag_row() {
        let t = Theme::default();
        let (fence, _) = parse(&["```", "code here", "```"]).unwrap();
        let lines = render(&fence, 40, &t);
        assert_eq!(lines.len(), 1);
        assert!(!line_text(&lines[0]).contains("rust"));
    }

    #[test]
    fn degenerate_narrow_width_falls_back_to_flat_lines() {
        let t = Theme::default();
        let (fence, _) = parse(&["```", "code", "```"]).unwrap();
        let lines = render(&fence, 2, &t);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "code");
    }
}
