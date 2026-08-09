//! GFM pipe tables.
//!
//! A table is a header row plus an immediately following delimiter row; the
//! block runs until a blank line or a line with no unescaped `|`. Column count
//! is fixed by the header — short body rows are padded, long ones truncated,
//! matching what github.com renders for the same body.
//!
//! Unlike the rest of [`super`], a table is *not* one output [`Line`] per source
//! line: cells wrap inside their column, so one source row occupies as many
//! screen rows as its tallest cell. Cell wrapping goes through
//! [`linkmap::wrap`], the same wrapper the detail pane wraps everything else
//! with, so the break rule cannot drift.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::super::linkmap;
use super::inline::parse_inline_links;
use super::{LinkSpan, Theme};

/// Narrowest a column may be squeezed to. One grapheme plus breathing room, so
/// wrapping inside the cell always makes progress.
const MIN_COL: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
    Center,
}

/// A parsed table: the delimiter row's alignments plus every row's cells,
/// header first. Every row has exactly `aligns.len()` cells.
pub(super) struct Table {
    aligns: Vec<Align>,
    rows: Vec<Vec<String>>,
}

/// One inline-parsed cell: styled spans, the links within them (columns
/// relative to the cell's own start), and the cell's rendered display width.
struct Cell {
    spans: Vec<Span<'static>>,
    links: Vec<LinkSpan>,
    width: usize,
}

/// Parse a table starting at `src[0]`, returning it with the number of source
/// lines consumed. `None` when `src` does not start a GFM table.
pub(super) fn parse(src: &[&str]) -> Option<(Table, usize)> {
    let header = split_cells(src.first()?)?;
    let aligns = delimiter_row(src.get(1)?)?;
    if aligns.len() != header.len() || header.is_empty() {
        return None;
    }

    let ncols = header.len();
    let mut rows = vec![fit_row(header, ncols)];
    let mut used = 2;
    while let Some(line) = src.get(used) {
        match split_cells(line) {
            Some(cells) => rows.push(fit_row(cells, ncols)),
            None => break,
        }
        used += 1;
    }

    Some((Table { aligns, rows }, used))
}

/// Pad with empty cells or truncate so the row matches the header's column
/// count — GFM's behaviour for ragged rows.
fn fit_row(mut cells: Vec<String>, ncols: usize) -> Vec<String> {
    cells.resize(ncols, String::new());
    cells
}

/// Split a table row on unescaped `|`, dropping the optional leading and
/// trailing pipes and trimming each cell. `None` when the line holds no
/// unescaped `|` at all, which is what ends a table block.
fn split_cells(line: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = line.chars().collect();
    let mut cells = vec![String::new()];
    let mut saw_pipe = false;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            // `\|` is a literal pipe inside a cell, not a separator. Other
            // escapes are left intact for the inline parser to handle.
            '\\' if chars.get(i + 1) == Some(&'|') => {
                cells.last_mut().expect("non-empty").push('|');
                i += 2;
            }
            '\\' if i + 1 < chars.len() => {
                let last = cells.last_mut().expect("non-empty");
                last.push('\\');
                last.push(chars[i + 1]);
                i += 2;
            }
            '|' => {
                saw_pipe = true;
                cells.push(String::new());
                i += 1;
            }
            c => {
                cells.last_mut().expect("non-empty").push(c);
                i += 1;
            }
        }
    }
    if !saw_pipe {
        return None;
    }

    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    Some(cells.into_iter().map(|c| c.trim().to_string()).collect())
}

/// Parse a delimiter row (`|---|:--:|---:|`) into per-column alignments.
fn delimiter_row(line: &str) -> Option<Vec<Align>> {
    let cells = split_cells(line)?;
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.ends_with(':');
            let dashes = cell.trim_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

impl Table {
    /// Render the table into `width` cells, reporting each link's position with
    /// `line` relative to the first line returned.
    pub(super) fn render(&self, width: usize, t: &Theme) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
        let cells: Vec<Vec<Cell>> = self
            .rows
            .iter()
            .map(|row| row.iter().map(|c| Cell::parse(c, t)).collect())
            .collect();

        let ncols = self.aligns.len();
        let natural: Vec<usize> = (0..ncols)
            .map(|c| cells.iter().map(|row| row[c].width).max().unwrap_or(0))
            .collect();
        // One leading space, then `" │ "` between each pair of columns.
        let budget = width.saturating_sub(1 + 3 * ncols.saturating_sub(1));
        let widths = fit_widths(&natural, budget);

        let mut out = Vec::new();
        let mut links = Vec::new();
        for (i, row) in cells.iter().enumerate() {
            self.render_row(row, &widths, i == 0, t, &mut out, &mut links);
            if i == 0 {
                out.push(rule_line(&widths, t));
            }
        }
        (out, links)
    }

    /// Append one source row's screen rows. The row is as tall as its tallest
    /// wrapped cell; shorter cells pad with blanks.
    fn render_row(
        &self,
        row: &[Cell],
        widths: &[usize],
        header: bool,
        t: &Theme,
        out: &mut Vec<Line<'static>>,
        links: &mut Vec<LinkSpan>,
    ) {
        let wrapped: Vec<(Vec<Line<'static>>, Vec<linkmap::LinkRect>)> = row
            .iter()
            .zip(widths)
            .map(|(cell, &w)| cell.wrap(w))
            .collect();
        let height = wrapped.iter().map(|(l, _)| l.len()).max().unwrap_or(1);
        let base = out.len();

        for r in 0..height {
            let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".to_string())];
            let mut x = 1usize;
            for (c, &w) in widths.iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled(" │ ".to_string(), Style::default().fg(t.dim)));
                    x += 3;
                }
                let empty = Vec::new();
                let content = wrapped[c].0.get(r);
                let row_spans = content.map_or(&empty, |l| &l.spans);
                let used: usize = row_spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                let pad = w.saturating_sub(used);
                let left = match self.aligns[c] {
                    Align::Left => 0,
                    Align::Right => pad,
                    Align::Center => pad / 2,
                };

                if left > 0 {
                    spans.push(Span::raw(" ".repeat(left)));
                }
                for s in row_spans {
                    let mut s = s.clone();
                    if header {
                        s.style = s.style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(s);
                }
                if pad > left {
                    spans.push(Span::raw(" ".repeat(pad - left)));
                }

                for rect in wrapped[c].1.iter().filter(|rect| rect.vrow == r) {
                    links.push(LinkSpan {
                        line: base + r,
                        col_start: x + left + rect.col_start,
                        col_end: x + left + rect.col_end,
                        url: rect.url.clone(),
                    });
                }
                x += w;
            }
            trim_trailing(&mut spans);
            out.push(Line::from(spans));
        }
    }
}

/// The `───┼───` rule under the header. Segments align each `┼` with the `│`
/// above it, and the rule ends flush with the widest a content row can be.
fn rule_line(widths: &[usize], t: &Theme) -> Line<'static> {
    let last = widths.len().saturating_sub(1);
    let rule: String = widths
        .iter()
        .enumerate()
        .map(|(i, &w)| "─".repeat(if i == last { w + 1 } else { w + 2 }))
        .collect::<Vec<_>>()
        .join("┼");
    Line::styled(rule, Style::default().fg(t.dim))
}

/// Drop trailing padding so a row carries no invisible tail — whole
/// whitespace-only spans first, then any spaces left on the final span (the
/// `" │ "` after a row's last cell is empty).
fn trim_trailing(spans: &mut Vec<Span<'static>>) {
    while let Some(last) = spans.last() {
        if last.content.chars().all(char::is_whitespace) {
            spans.pop();
        } else {
            break;
        }
    }
    if let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end();
        if trimmed.len() != last.content.len() {
            last.content = trimmed.to_string().into();
        }
    }
}

/// Fair-share water-fill: every column gets `min(natural, level)` for the
/// highest level that fits `budget`, so columns already narrower than their
/// share keep their natural width and the surplus goes to the wide ones.
/// Columns floor at [`MIN_COL`] (never widened past their natural width), which
/// may push the table over `budget` — deliberately, rather than collapsing to
/// unreadable single-character columns.
fn fit_widths(natural: &[usize], budget: usize) -> Vec<usize> {
    let total: usize = natural.iter().sum();
    if total <= budget {
        return natural.to_vec();
    }

    let (mut lo, mut hi) = (0usize, natural.iter().copied().max().unwrap_or(0));
    while lo < hi {
        let mid = lo.midpoint(hi) + 1;
        let fits: usize = natural.iter().map(|&n| n.min(mid)).sum();
        if fits <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut widths: Vec<usize> = natural.iter().map(|&n| n.min(lo)).collect();
    // Integer division leaves a remainder; hand it to the capped columns.
    let mut spare = budget.saturating_sub(widths.iter().sum::<usize>());
    for (w, &n) in widths.iter_mut().zip(natural) {
        if spare == 0 {
            break;
        }
        if *w < n {
            *w += 1;
            spare -= 1;
        }
    }

    for (w, &n) in widths.iter_mut().zip(natural) {
        *w = n.min((*w).max(MIN_COL));
    }
    widths
}

impl Cell {
    fn parse(text: &str, t: &Theme) -> Self {
        let (spans, locals) = parse_inline_links(text, t);
        let width = spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let links = locals
            .into_iter()
            .map(|l| LinkSpan {
                line: 0,
                col_start: l.start,
                col_end: l.end,
                url: l.url,
            })
            .collect();
        Self {
            spans,
            links,
            width,
        }
    }

    /// Wrap the cell into `width` cells using the detail pane's own wrapper, so
    /// table cells break exactly like every other wrapped line.
    fn wrap(&self, width: usize) -> (Vec<Line<'static>>, Vec<linkmap::LinkRect>) {
        let line = Line::from(self.spans.clone());
        linkmap::wrap(&[line], &self.links, width)
    }
}

#[cfg(test)]
mod tests {
    use super::super::render_with_links;
    use crate::tui::theme::Theme;
    use ratatui::text::Line;

    fn render(body: &str, width: usize) -> Vec<String> {
        let t = Theme::default();
        render_with_links(body, width, &t)
            .0
            .iter()
            .map(text)
            .collect()
    }

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn header_delimiter_and_body_render_as_a_table() {
        let out = render(
            "| Repo | Status |\n|------|--------|\n| homelabia | open |",
            80,
        );
        assert_eq!(
            out,
            vec![
                " Repo      │ Status",
                "───────────┼───────",
                " homelabia │ open",
            ]
        );
    }

    #[test]
    fn header_without_a_delimiter_row_stays_plain_text() {
        let out = render("| a | b |\nplain text", 80);
        assert_eq!(out, vec!["| a | b |", "plain text"]);
    }

    #[test]
    fn delimiter_column_count_must_match_the_header() {
        let out = render("| a | b |\n|---|", 80);
        assert_eq!(out, vec!["| a | b |", "|---|"]);
    }

    #[test]
    fn ragged_rows_are_padded_and_truncated_to_the_header() {
        let out = render(
            "| Repo | Status | Notes |\n|------|--------|-------|\n| homelabia | open |\n| tui | closed | shipped | extra |",
            80,
        );
        assert_eq!(
            out,
            vec![
                " Repo      │ Status │ Notes",
                "───────────┼────────┼────────",
                " homelabia │ open   │",
                " tui       │ closed │ shipped",
            ]
        );
    }

    #[test]
    fn alignment_markers_pad_left_right_and_centre() {
        let out = render(
            "| Item | Qty | Note |\n|:-----|----:|:----:|\n| widget | 12 | ok |\n| gadget | 3 | urgent |",
            80,
        );
        assert_eq!(
            out,
            vec![
                " Item   │ Qty │  Note",
                "────────┼─────┼───────",
                " widget │  12 │   ok",
                " gadget │   3 │ urgent",
            ]
        );
    }

    #[test]
    fn narrow_pane_water_fills_widths_and_wraps_cells() {
        let out = render(
            "| Repo | Status | Notes |\n|------|--------|-------|\n| homelabia | open | needs a regression test |",
            30,
        );
        // Natural widths 9/6/23 exceed the 23-cell budget; the two columns that
        // already fit keep their width and the surplus goes to Notes.
        assert_eq!(
            out,
            vec![
                " Repo      │ Status │ Notes",
                "───────────┼────────┼─────────",
                " homelabia │ open   │ needs a",
                "           │        │ regressi",
                "           │        │ on test",
            ]
        );
    }

    #[test]
    fn escaped_pipe_is_a_literal_cell_character() {
        let out = render("| a | b |\n|---|---|\n| x \\| y | z |", 80);
        assert_eq!(out, vec![" a     │ b", "───────┼──", " x | y │ z"]);
    }

    #[test]
    fn wide_glyphs_measure_by_display_width() {
        let out = render("| 名前 | b |\n|------|---|\n| x | y |", 80);
        assert_eq!(out, vec![" 名前 │ b", "──────┼──", " x    │ y"]);
    }

    #[test]
    fn a_table_inside_a_fence_is_not_parsed() {
        // Fence delimiters are dropped and each content line gets a gutter
        // prefix (#120), but the pipe syntax stays literal — it must not be
        // read as a table.
        let out = render("```\n| a |\n|---|\n```", 80);
        assert_eq!(out.len(), 2);
        for (row, content) in out.iter().zip(["| a |", "|---|"]) {
            assert_eq!(row.trim_end(), format!("▏ {content}"));
        }
    }

    #[test]
    fn columns_floor_at_three_cells_and_never_bail() {
        let body = format!(
            "|{}\n|{}\n|{}",
            " aaaa |".repeat(10),
            "------|".repeat(10),
            " bbbb |".repeat(10)
        );
        let out = render(&body, 46);
        // Header and body each wrap to two rows at a 3-cell column width.
        assert_eq!(out.len(), 5);
        assert_eq!(
            out[0],
            " aaa │ aaa │ aaa │ aaa │ aaa │ aaa │ aaa │ aaa │ aaa │ aaa"
        );
        // Deliberately wider than the 46-cell pane rather than collapsing.
        assert_eq!(out[0].chars().count(), 58);
    }

    #[test]
    fn link_inside_a_cell_reports_its_position() {
        let t = Theme::default();
        let (_, links) = render_with_links(
            "| Link |\n|------|\n| [docs](https://example.com) |",
            80,
            &t,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com");
        assert_eq!(links[0].line, 2);
        assert_eq!((links[0].col_start, links[0].col_end), (1, 5));
    }

    #[test]
    fn link_after_a_table_is_indexed_against_the_expanded_output() {
        let t = Theme::default();
        let body = "| a |\n|---|\n| one two three four |\n\nsee https://example.com";
        let (lines, links) = render_with_links(body, 12, &t);
        // The single body row wraps to two screen rows, so the table emits four
        // lines from three source lines.
        assert_eq!(lines.len(), 6);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].line, 5);
        assert_eq!(links[0].col_start, 4);
    }
}
