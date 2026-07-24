//! Word-wrap the detail pane's lines *and* track where each URL lands.
//!
//! The detail regions used to lean on `Paragraph`'s internal `Wrap` for both
//! rendering and (via `Paragraph::line_count`) scroll measurement. To make URLs
//! clickable we need the exact wrapped cell positions of each link, which that
//! internal wrapper does not expose. So we own the wrap here: [`wrap`] returns
//! already-wrapped lines (rendered by a `Paragraph` with wrapping *off*) plus a
//! [`LinkRect`] per URL run, and [`wrapped_height`] counts rows the same way.
//! Both go through [`row_ranges`], so measured and drawn heights always agree.
//!
//! The break rule mirrors the editor's [`super::app::wrap_lines`]: break after
//! the last whitespace that fits the window, hard-break words longer than the
//! width. Widths are in terminal cells (display width), so wide glyphs and the
//! link columns reported by [`super::markdown`] line up 1:1 with buffer columns.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::markdown::LinkSpan;

/// A URL occupying a run of cells on one wrapped visual row. Links that span a
/// wrap boundary yield several rects sharing one `id` so terminals treat them
/// as a single hyperlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRect {
    /// Visual-row index within the wrapped output.
    pub vrow: usize,
    /// Column range within that row (cells, `start..end`).
    pub col_start: usize,
    pub col_end: usize,
    pub url: String,
    /// Stable per-link id (index into the input `links`).
    pub id: usize,
}

/// One grapheme cluster with its cell width, style, and owning link (if any).
struct Grapheme {
    sym: String,
    width: usize,
    style: Style,
    link: Option<usize>,
}

fn cell_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Expand a rendered line into grapheme clusters, tagging each with the link id
/// (index into `links`) whose column range covers it.
fn expand(line: &Line<'_>, line_idx: usize, links: &[LinkSpan]) -> Vec<Grapheme> {
    let mut out = Vec::new();
    let mut col = 0usize;
    for span in &line.spans {
        for g in span.content.as_ref().graphemes(true) {
            let width = cell_width(g);
            let link = links
                .iter()
                .position(|l| l.line == line_idx && col >= l.col_start && col < l.col_end);
            out.push(Grapheme {
                sym: g.to_string(),
                width,
                style: span.style,
                link,
            });
            col += width;
        }
    }
    out
}

/// Split a line's graphemes into visual rows at `width` cells. Returns index
/// ranges into `gs`. An empty line yields a single empty row.
fn row_ranges(gs: &[Grapheme], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        // Does everything from `start` fit on one row?
        let remaining: usize = gs[start..].iter().map(|g| g.width).sum();
        if remaining <= width {
            rows.push((start, gs.len()));
            break;
        }
        // Largest prefix of gs[start..] whose width fits the window.
        let mut acc = 0;
        let mut window_end = start;
        while window_end < gs.len() && acc + gs[window_end].width <= width {
            acc += gs[window_end].width;
            window_end += 1;
        }
        // A single grapheme wider than the whole width: emit it alone so we
        // always make progress.
        if window_end == start {
            window_end = start + 1;
        }
        // Break after the last whitespace in the window (it stays on this row);
        // otherwise hard-break at the window edge.
        let brk = (start..window_end)
            .rev()
            .find(|&i| is_whitespace(&gs[i]))
            .map(|i| i + 1)
            .unwrap_or(window_end);
        rows.push((start, brk));
        start = brk;
    }
    rows
}

fn is_whitespace(g: &Grapheme) -> bool {
    !g.sym.is_empty() && g.sym.chars().all(char::is_whitespace)
}

/// Merge a run of graphemes into as few same-style spans as possible.
fn build_line(gs: &[Grapheme]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for g in gs {
        match spans.last_mut() {
            Some(last) if last.style == g.style => last.content.to_mut().push_str(&g.sym),
            _ => spans.push(Span::styled(g.sym.clone(), g.style)),
        }
    }
    Line::from(spans)
}

/// Collect the link rects on one wrapped row (`gs[range]` at output row `vrow`).
fn row_links(gs: &[Grapheme], vrow: usize, links: &[LinkSpan], out: &mut Vec<LinkRect>) {
    let mut col = 0usize;
    let mut run: Option<(usize, usize, usize)> = None; // (id, col_start, col_end)
    for g in gs {
        match (run, g.link) {
            (Some((id, start, _end)), Some(lid)) if id == lid => {
                run = Some((id, start, col + g.width));
            }
            (Some((id, start, end)), _) => {
                push_rect(out, vrow, start, end, id, links);
                run = g.link.map(|lid| (lid, col, col + g.width));
            }
            (None, Some(lid)) => run = Some((lid, col, col + g.width)),
            (None, None) => {}
        }
        col += g.width;
    }
    if let Some((id, start, end)) = run {
        push_rect(out, vrow, start, end, id, links);
    }
}

fn push_rect(
    out: &mut Vec<LinkRect>,
    vrow: usize,
    col_start: usize,
    col_end: usize,
    id: usize,
    links: &[LinkSpan],
) {
    out.push(LinkRect {
        vrow,
        col_start,
        col_end,
        url: links[id].url.clone(),
        id,
    });
}

/// Wrap `lines` at `width` cells, returning the wrapped lines (for a
/// wrapping-off `Paragraph`) and one [`LinkRect`] per URL run.
pub fn wrap(
    lines: &[Line<'static>],
    links: &[LinkSpan],
    width: usize,
) -> (Vec<Line<'static>>, Vec<LinkRect>) {
    let mut out_lines = Vec::new();
    let mut rects = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let gs = expand(line, li, links);
        for (start, end) in row_ranges(&gs, width) {
            let vrow = out_lines.len();
            let slice = &gs[start..end];
            if !links.is_empty() {
                row_links(slice, vrow, links, &mut rects);
            }
            out_lines.push(build_line(slice));
        }
    }
    (out_lines, rects)
}

/// Number of wrapped visual rows `lines` occupy at `width` cells. Matches the
/// row count [`wrap`] produces, so scroll clamps stay in sync with rendering.
pub fn wrapped_height(lines: &[Line<'static>], width: usize) -> u16 {
    let count: usize = lines
        .iter()
        .enumerate()
        .map(|(li, line)| row_ranges(&expand(line, li, &[]), width).len())
        .sum();
    u16::try_from(count).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Line<'static>> {
        text.lines().map(|l| Line::from(l.to_string())).collect()
    }

    #[test]
    fn short_lines_are_one_row_each() {
        let ls = lines("abc\ndef");
        assert_eq!(wrapped_height(&ls, 80), 2);
        let (wrapped, rects) = wrap(&ls, &[], 80);
        assert_eq!(wrapped.len(), 2);
        assert!(rects.is_empty());
    }

    #[test]
    fn empty_line_is_one_row() {
        let ls = vec![Line::default()];
        assert_eq!(wrapped_height(&ls, 80), 1);
        assert_eq!(wrap(&ls, &[], 80).0.len(), 1);
    }

    #[test]
    fn long_unbroken_token_hard_breaks() {
        let ls = lines(&"x".repeat(25));
        // 25 chars at width 10 → rows of 10, 10, 5.
        assert_eq!(wrapped_height(&ls, 10), 3);
    }

    #[test]
    fn breaks_after_last_whitespace_in_window() {
        let ls = lines("alpha beta gamma");
        // width 12: "alpha beta " fits (11), "gamma" wraps → 2 rows.
        let (wrapped, _) = wrap(&ls, &[], 12);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(row_text(&wrapped[0]), "alpha beta ");
        assert_eq!(row_text(&wrapped[1]), "gamma");
    }

    #[test]
    fn height_matches_wrap_row_count() {
        let ls = lines("a much longer line that will certainly wrap several times over");
        for w in [8usize, 13, 20, 40] {
            assert_eq!(wrapped_height(&ls, w) as usize, wrap(&ls, &[], w).0.len());
        }
    }

    #[test]
    fn link_rect_tracks_a_bare_url_position() {
        let ls = lines("see https://example.com now");
        let links = vec![LinkSpan {
            line: 0,
            col_start: 4,
            col_end: 23,
            url: "https://example.com".into(),
        }];
        let (_, rects) = wrap(&ls, &links, 80);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].vrow, 0);
        assert_eq!(rects[0].col_start, 4);
        assert_eq!(rects[0].col_end, 23);
        assert_eq!(rects[0].url, "https://example.com");
    }

    #[test]
    fn wrapped_link_yields_multiple_rects_sharing_id() {
        // A URL forced across a wrap boundary produces one rect per row, same id.
        let url = "https://example.com/aaaa/bbbb/cccc";
        let ls = lines(url);
        let links = vec![LinkSpan {
            line: 0,
            col_start: 0,
            col_end: url.chars().count(),
            url: url.into(),
        }];
        let (wrapped, rects) = wrap(&ls, &links, 20);
        assert!(wrapped.len() >= 2);
        assert!(rects.len() >= 2);
        assert!(rects.iter().all(|r| r.id == 0));
        assert!(rects.iter().all(|r| r.url == url));
        // Rects cover distinct rows.
        assert_eq!(rects[0].vrow, 0);
        assert_eq!(rects[1].vrow, 1);
    }

    fn row_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}
