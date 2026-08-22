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
use super::highlight::{self, LangSpec, State};

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

    // Highlighting keys off the info string, not off whether the tag was
    // actually drawn — the tag is dropped on a width collision (#120), and
    // that must not silently turn colour off. `None` (unknown language, or no
    // info string) yields one Text segment per line, i.e. pre-#122 output.
    let spec = fence.lang.as_deref().and_then(highlight::spec_for);
    let mut state = State::Normal;

    let mut out = Vec::new();
    for line in &fence.content {
        let segments = segment(line, spec, &mut state, t);
        for chunk in hard_break(&segments, code_area) {
            let chunk_w: usize = chunk
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let mut spans = vec![Span::styled(GUTTER, gutter_style)];

            let tag_drawn = out.is_empty()
                && tag.as_ref().is_some_and(|tag| {
                    let tag_w = UnicodeWidthStr::width(tag.as_str());
                    code_area.saturating_sub(chunk_w) >= tag_w
                });

            spans.extend(chunk);
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

/// Split one source line into styled `(text, style)` runs.
///
/// The whole line is tokenised *before* [`hard_break`] chops it at the pane
/// edge, so a token straddling the boundary keeps one colour across both rows
/// and resizing the pane never recolours code.
fn segment(
    line: &str,
    spec: Option<&'static LangSpec>,
    state: &mut State,
    t: &Theme,
) -> Vec<(String, Style)> {
    let Some(spec) = spec else {
        return vec![(line.to_string(), super::code_style(t))];
    };
    highlight::tokenize(line, spec, state)
        .into_iter()
        .map(|(r, kind)| (line[r].to_string(), super::token_style(t, kind)))
        .collect()
}

/// Split styled `segments` into rows of at most `width` display cells,
/// breaking strictly at the width boundary (never at whitespace) and splitting
/// a segment when the boundary falls inside it. A single grapheme wider than
/// `width` is emitted alone so progress is always made. An empty line yields
/// one empty row.
fn hard_break(segments: &[(String, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut buf = String::new();

    for (text, style) in segments {
        for g in text.graphemes(true) {
            let gw = UnicodeWidthStr::width(g);
            // Never break an empty row, or a grapheme wider than the pane
            // would loop forever.
            if cur_w + gw > width && !(cur.is_empty() && buf.is_empty()) {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), *style));
                }
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            buf.push_str(g);
            cur_w += gw;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(std::mem::take(&mut buf), *style));
        }
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

    /// A row's code text: everything after the gutter, trailing pad removed.
    /// Highlighting splits the code into several spans, so this can no longer
    /// read a single span.
    fn code_text(l: &Line<'_>) -> String {
        let joined: String = l.spans.iter().skip(1).map(|s| s.content.as_ref()).collect();
        joined.trim_end().to_string()
    }

    /// Every span that is actually code: gutter dropped, and the italic
    /// language tag — which is chrome, not content — dropped with it.
    fn code_spans<'a>(lines: &'a [Line<'a>]) -> Vec<&'a Span<'a>> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().skip(1))
            .filter(|s| !s.style.add_modifier.contains(Modifier::ITALIC))
            .collect()
    }

    fn row_width(l: &Line<'_>) -> usize {
        l.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
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
        let rebuilt: String = lines.iter().map(code_text).collect();
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

    // --- syntax highlighting (#122) ------------------------------------

    #[test]
    fn highlighted_rows_are_still_exactly_width_cells() {
        // The span-splitting break is the thing most likely to break the
        // #120 row-width invariant, so re-pin it with highlighting on.
        let t = Theme::default();
        let (fence, _) = parse(&[
            "```rust",
            "let total = items.iter().map(|i| i.price).sum::<f64>(); // note",
            "",
            "let s = \"a string that is quite long indeed\";",
            "```",
        ])
        .unwrap();
        for width in [10usize, 20, 40, 80] {
            for l in &render(&fence, width, &t) {
                assert_eq!(row_width(l), width, "row width mismatch at width {width}");
            }
        }
    }

    #[test]
    fn tokens_are_coloured_by_kind() {
        let t = Theme::default();
        let (fence, _) = parse(&["```rust", "let n = 42; // note", "```"]).unwrap();
        let lines = render(&fence, 60, &t);
        let spans = code_spans(&lines);
        let fg_of = |needle: &str| {
            spans
                .iter()
                .find(|s| s.content.as_ref() == needle)
                .unwrap_or_else(|| panic!("no span {needle:?}"))
                .style
                .fg
        };
        assert_eq!(fg_of("let"), Some(t.code_keyword));
        assert_eq!(fg_of("42"), Some(t.code_number));
        assert_eq!(fg_of("// note"), Some(t.code_comment));
        // Every code span keeps the code background, so the fill is unbroken.
        assert!(spans.iter().all(|s| s.style.bg == Some(t.code_bg)));
    }

    #[test]
    fn a_token_split_by_the_hard_break_keeps_its_colour_on_both_rows() {
        let t = Theme::default();
        let long_string = "a".repeat(40);
        let src = format!("let s = \"{long_string}\";");
        let (fence, _) = parse(&["```rust", &src, "```"]).unwrap();
        let lines = render(&fence, 22, &t); // code_area = 20, splits the string
        assert!(lines.len() > 1, "expected the line to break");
        for s in code_spans(&lines) {
            if s.content.contains('a') {
                assert_eq!(
                    s.style.fg,
                    Some(t.code_string),
                    "string fragment {:?} lost its colour",
                    s.content
                );
            }
        }
    }

    #[test]
    fn colours_do_not_change_with_pane_width() {
        // The whole point of tokenising before breaking: resizing must not
        // recolour anything.
        let t = Theme::default();
        let (fence, _) = parse(&["```rust", "let msg = \"hello world there\";", "```"]).unwrap();
        let colour_run = |width: usize| -> Vec<(char, Option<ratatui::style::Color>)> {
            let lines = render(&fence, width, &t);
            code_spans(&lines)
                .iter()
                .flat_map(|s| {
                    s.content
                        .chars()
                        .map(|c| (c, s.style.fg))
                        .collect::<Vec<_>>()
                })
                .filter(|(c, _)| !c.is_whitespace())
                .collect()
        };
        assert_eq!(colour_run(80), colour_run(24));
    }

    #[test]
    fn unknown_language_keeps_the_flat_pre_highlighting_styling() {
        let t = Theme::default();
        let (fence, _) = parse(&["```mermaid", "graph TD; A-->B; 42", "```"]).unwrap();
        let lines = render(&fence, 60, &t);
        for s in code_spans(&lines) {
            assert_eq!(
                s.style.fg,
                Some(t.code_fg),
                "span {:?} was styled",
                s.content
            );
        }
    }

    #[test]
    fn bare_fence_is_unhighlighted() {
        let t = Theme::default();
        let (fence, _) = parse(&["```", "let n = 42;", "```"]).unwrap();
        for s in code_spans(&render(&fence, 60, &t)) {
            assert_eq!(s.style.fg, Some(t.code_fg));
        }
    }

    #[test]
    fn highlighting_survives_the_language_tag_being_dropped() {
        let t = Theme::default();
        let long_first = format!("let {} = 1;", "x".repeat(30));
        let (fence, _) = parse(&["```rust", &long_first, "```"]).unwrap();
        let lines = render(&fence, 20, &t);
        // Tag did not fit...
        assert!(!line_text(&lines[0]).contains("rust"));
        // ...but the code is still coloured.
        assert!(
            code_spans(&lines)
                .iter()
                .any(|s| s.style.fg == Some(t.code_keyword))
        );
    }

    #[test]
    fn block_comment_state_carries_within_a_fence_but_not_between_fences() {
        let t = Theme::default();
        let (open, _) = parse(&["```go", "/* one", "two", "*/", "```"]).unwrap();
        let lines = render(&open, 40, &t);
        for (i, l) in lines.iter().enumerate() {
            assert!(
                code_spans(&lines[i..=i])
                    .iter()
                    .filter(|s| !s.content.trim().is_empty())
                    .all(|s| s.style.fg == Some(t.code_comment)),
                "row {i} ({:?}) escaped the block comment",
                line_text(l)
            );
        }

        // A second fence starts from a clean State::Normal.
        let (next, _) = parse(&["```go", "var x = 1", "```"]).unwrap();
        assert!(
            code_spans(&render(&next, 40, &t))
                .iter()
                .any(|s| s.style.fg == Some(t.code_keyword))
        );
    }

    #[test]
    fn degenerate_narrow_width_is_flat_even_for_a_known_language() {
        let t = Theme::default();
        let (fence, _) = parse(&["```rust", "let n = 1;", "```"]).unwrap();
        let lines = render(&fence, 2, &t);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "let n = 1;");
    }
}
