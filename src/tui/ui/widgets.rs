use super::prelude::*;

/// GitHub label colors arrive as 6-digit hex without `#`.
pub(super) fn inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

/// Wrap each link's on-screen cells in an OSC 8 hyperlink escape so terminals
/// make them clickable (Ctrl/Cmd+Click), opening the URL in the default
/// browser. The visible glyph and style are preserved; `ForcedWidth` pins each
/// touched cell's real display width so the escape bytes don't disturb ratatui's
/// layout/diff. Rects are given in unscrolled content coordinates; `scroll` and
/// the viewport clip them to what's visible.
pub(super) fn apply_hyperlinks(buf: &mut Buffer, inner: Rect, rects: &[LinkRect], scroll: u16) {
    for r in rects {
        let vrow = r.vrow as u16;
        if vrow < scroll {
            continue;
        }
        let row = vrow - scroll;
        if row >= inner.height {
            continue;
        }
        let y = inner.y + row;
        let start = r.col_start as u16;
        if start >= inner.width {
            continue;
        }
        let end = (r.col_end as u16).min(inner.width);
        if end <= start {
            continue;
        }
        for x in start..end {
            let is_first = x == start;
            let is_last = x == end - 1;
            if !is_first && !is_last {
                continue; // interior cells stay inside the open link
            }
            let Some(cell) = buf.cell_mut((inner.x + x, y)) else {
                continue;
            };
            let glyph = cell.symbol().to_string();
            let width = UnicodeWidthStr::width(glyph.as_str()).max(1) as u16;
            let mut sym = String::new();
            if is_first {
                sym.push_str(&format!("\x1b]8;id={};{}\x1b\\", r.id, r.url));
            }
            sym.push_str(&glyph);
            if is_last {
                sym.push_str("\x1b]8;;\x1b\\");
            }
            cell.set_symbol(&sym);
            cell.set_diff_option(CellDiffOption::ForcedWidth(
                NonZeroU16::new(width).unwrap_or(NonZeroU16::MIN),
            ));
        }
    }
}

/// Draw a vertical scrollbar on `area`'s right edge when `content_h` overflows
/// `viewport_h`; a no-op otherwise so short content stays uncluttered.
pub(super) fn render_region_scrollbar(
    f: &mut Frame,
    t: &Theme,
    area: Rect,
    content_h: u16,
    viewport_h: u16,
    pos: u16,
) {
    if content_h <= viewport_h {
        return;
    }
    let mut state = ScrollbarState::new(content_h as usize)
        .viewport_content_length(viewport_h as usize)
        .position(pos as usize);
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(t.accent))
            .track_style(Style::default().fg(t.dim)),
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

/// Metadata header + rendered description for the body region. The title is
/// highlighted when the body is the selected region. Shared by the renderer
/// Wrapped (visual) height of `lines` at inner width `width`, measured with
/// the same [`linkmap`] wrapper the regions render with.
pub(super) fn paragraph_height(lines: &[Line<'static>], width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    linkmap::wrapped_height(lines, width as usize)
}

/// Wrapped height of the body region's content (metadata + description) at
/// inner width `width`. Styling doesn't affect wrapping, so a default theme
/// GitHub label colours arrive as 6-digit hex without `#`.
pub(super) fn label_color(hex: &str, fallback: Color) -> Color {
    if hex.len() == 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        )
    {
        return Color::Rgb(r, g, b);
    }
    fallback
}

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}
