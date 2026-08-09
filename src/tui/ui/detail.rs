use super::form::draw_comment_section;
use super::list::pane_border;
use super::list::{state_style, title_style};
use super::prelude::*;
use super::widgets::{apply_hyperlinks, inner_area, paragraph_height, render_region_scrollbar};
use crate::tui::app::{DetailSel, Focus, Mode};
use crate::tui::markdown;
use crate::tui::markdown::LinkSpan;

pub(super) fn draw_detail(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    // The inline comment editor, when open, takes the bottom third of the pane;
    // the body + comments regions share the rest so the body stays visible.
    let area = if app.mode == Mode::CommentEditor {
        let [thread, comment] =
            Layout::vertical([Constraint::Min(1), Constraint::Percentage(33)]).areas(area);
        draw_comment_section(f, app, t, comment);
        thread
    } else {
        area
    };

    let Some(issue) = app.selected_issue() else {
        // Live follow landed on a repo header (or an empty list).
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(pane_border(app, t, Focus::Detail))
            .title(" issue ");
        f.render_widget(
            Paragraph::new(Line::styled(
                "no issue selected",
                Style::default().fg(t.dim),
            ))
            .block(block),
            area,
        );
        return;
    };

    let focused = app.focus == Focus::Detail;
    let regions = layout::detail_regions(area);

    draw_detail_body(f, app, t, issue, regions.body, focused);
    if let Some(comments) = regions.comments {
        draw_detail_comments(f, app, t, comments, focused);
    }
}

/// The fixed top region: issue metadata + the description body, scrolled by
/// `body_scroll`, with a scrollbar when the content overflows.
pub(super) fn draw_detail_body(
    f: &mut Frame,
    app: &App,
    t: &Theme,
    issue: &Issue,
    area: Rect,
    focused: bool,
) {
    let selected = focused && app.detail.sel == DetailSel::Body;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(" issue (Tab comment · j/k scroll · e edit · P PR · ← list · Esc close) ");
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    let (lines, links) = body_lines_links(issue, selected, inner_w as usize, t);
    let (wrapped, rects) = linkmap::wrap(&lines, &links, inner_w as usize);
    let content_h = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);
    let max_scroll = content_h.saturating_sub(inner_h);
    let scroll = app.detail.body_scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(wrapped).block(block).scroll((scroll, 0)),
        area,
    );
    render_region_scrollbar(f, t, area, content_h, inner_h, scroll);
    apply_hyperlinks(f.buffer_mut(), inner_area(area), &rects, scroll);
}

/// The bottom region: the stacked comment cards, scrolled by `comments_scroll`,
/// with a scrollbar reflecting position within the *selected* comment.
pub(super) fn draw_detail_comments(f: &mut Frame, app: &App, t: &Theme, area: Rect, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(" comments ");
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    let selected = match app.detail.sel {
        _ if !focused => None,
        DetailSel::Comment(i) => Some(i),
        DetailSel::Body => None,
    };

    let comments = match &app.detail.comments {
        None => {
            f.render_widget(
                Paragraph::new(Line::styled(
                    "loading comments…",
                    Style::default().fg(t.dim),
                ))
                .block(block),
                area,
            );
            return;
        }
        Some(c) if c.is_empty() => {
            f.render_widget(
                Paragraph::new(Line::styled("no comments", Style::default().fg(t.dim)))
                    .block(block),
                area,
            );
            return;
        }
        Some(c) => c,
    };

    let card_width = inner_w as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut links: Vec<LinkSpan> = Vec::new();
    for (i, c) in comments.iter().enumerate() {
        let (card, card_links) = comment_card_lines_links(c, selected == Some(i), card_width, t);
        let base = lines.len();
        for mut l in card_links {
            l.line += base;
            links.push(l);
        }
        lines.extend(card);
    }
    let (wrapped, rects) = linkmap::wrap(&lines, &links, inner_w as usize);
    let total = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);

    let scroll = app.detail.comments_scroll;
    f.render_widget(
        Paragraph::new(wrapped).block(block).scroll((scroll, 0)),
        area,
    );

    // Scrollbar maps to the selected comment's own extent; falls back to the
    // whole thread when the body (not a comment) has focus.
    if let Some(i) = selected {
        let top = comment_offset(comments, i, inner_w);
        let height = comment_height(&comments[i], inner_w);
        render_region_scrollbar(f, t, area, height, inner_h, scroll.saturating_sub(top));
    } else {
        render_region_scrollbar(f, t, area, total, inner_h, scroll);
    }
    apply_hyperlinks(f.buffer_mut(), inner_area(area), &rects, scroll);
}

/// The issue metadata + description lines, without link positions.
pub(super) fn body_lines(
    issue: &Issue,
    selected: bool,
    width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    body_lines_links(issue, selected, width, t).0
}

/// [`body_lines`] plus the URL positions in the description, with each link's
/// line index offset past the metadata header so it points into the returned
/// `Vec<Line>`.
pub(super) fn body_lines_links(
    issue: &Issue,
    selected: bool,
    width: usize,
    t: &Theme,
) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
    let mut title_style = title_style(issue, t).add_modifier(Modifier::BOLD);
    if selected {
        title_style = title_style.fg(t.accent).add_modifier(Modifier::REVERSED);
    }
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("#{} ", issue.number), Style::default().fg(t.dim)),
            Span::styled(issue.title.clone(), title_style),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", issue.state), state_style(issue, t)),
            Span::styled(
                format!(
                    "by {} · created {} · updated {}{}",
                    issue.author,
                    issue.created_at.format("%Y-%m-%d"),
                    issue.updated_at.format("%Y-%m-%d"),
                    issue
                        .closed_at
                        .map(|c| format!(" · closed {}", c.format("%Y-%m-%d")))
                        .unwrap_or_default(),
                ),
                Style::default().fg(t.dim),
            ),
        ]),
        Line::from(Span::styled(
            format!(
                "assignees: {}   labels: {}",
                if issue.assignees.is_empty() {
                    "—".to_string()
                } else {
                    issue.assignees.join(", ")
                },
                if issue.labels.is_empty() {
                    "—".to_string()
                } else {
                    issue
                        .labels
                        .iter()
                        .map(|l| l.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ),
            Style::default().fg(t.assignee),
        )),
        Line::default(),
    ];
    let base = lines.len();
    let (md_lines, md_links) = markdown::render_with_links(&issue.body, width, t);
    let links = md_links
        .into_iter()
        .map(|mut l| {
            l.line += base;
            l
        })
        .collect();
    lines.extend(md_lines);
    (lines, links)
}

/// One comment card: an author·timestamp header rule, the rendered body, a
/// bottom rule, and a trailing blank separator. Highlighted (accent/reversed)
/// when it is the selected card. Shared by the renderer and `comment_height`.
pub(super) fn comment_card_lines(
    c: &Comment,
    selected: bool,
    card_width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    comment_card_lines_links(c, selected, card_width, t).0
}

/// [`comment_card_lines`] plus the URL positions in the comment body, with each
/// link's line index offset past the header rule.
pub(super) fn comment_card_lines_links(
    c: &Comment,
    selected: bool,
    card_width: usize,
    t: &Theme,
) -> (Vec<Line<'static>>, Vec<LinkSpan>) {
    let header = format!(
        "── {} · {} ",
        c.author,
        c.created_at.format("%Y-%m-%d %H:%M")
    );
    let mut header_style = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let mut rule_style = Style::default().fg(t.dim);
    if selected {
        header_style = header_style.add_modifier(Modifier::REVERSED);
        rule_style = Style::default().fg(t.accent);
    }
    let mut lines = vec![rule_line(&header, card_width, header_style)];
    let base = lines.len();
    let (md_lines, md_links) = markdown::render_with_links(&c.body, card_width, t);
    let links = md_links
        .into_iter()
        .map(|mut l| {
            l.line += base;
            l
        })
        .collect();
    lines.extend(md_lines);
    lines.push(rule_line("", card_width, rule_style));
    lines.push(Line::default());
    (lines, links)
}

/// Wrapped (visual) height of the body region's content at inner width
/// `width`, measured with the same wrapper the region renders with so the
/// scroll clamps match the drawn rows.
pub fn body_content_height(issue: &Issue, width: u16) -> u16 {
    paragraph_height(
        &body_lines(issue, false, width as usize, &Theme::default()),
        width,
    )
}

/// Wrapped height of one comment card (header rule + body + footer + blank) at
/// inner width `width`.
pub fn comment_height(c: &Comment, width: u16) -> u16 {
    paragraph_height(
        &comment_card_lines(c, false, width as usize, &Theme::default()),
        width,
    )
}

/// Visual-row offset of comment `i`'s top within the stacked comments
/// paragraph: the summed heights of the comments before it.
pub fn comment_offset(comments: &[Comment], i: usize, width: u16) -> u16 {
    comments
        .iter()
        .take(i)
        .map(|c| comment_height(c, width))
        .fold(0u16, |acc, h| acc.saturating_add(h))
}

/// A horizontal card rule: `prefix` followed by box-drawing dashes filling out
/// to `width`, all in `style`. Used for the comment cards' header and footer.
pub(super) fn rule_line(prefix: &str, width: usize, style: Style) -> Line<'static> {
    let fill = width.saturating_sub(prefix.chars().count());
    Span::styled(format!("{prefix}{}", "─".repeat(fill)), style).into()
}

#[cfg(test)]
mod tests {
    use super::super::draw;
    use super::super::testutil::*;
    use super::*;
    use crate::tui::app::DetailSel;

    #[test]
    fn body_content_height_counts_metadata_and_body() {
        // 4 metadata lines (title, state, assignees, blank) + 0 body lines.
        let empty = issue(vec![]);
        assert_eq!(body_content_height(&empty, 80), 4);
        // + three body lines, none wide enough to wrap at width 80.
        let mut three = issue(vec![]);
        three.body = "line one\nline two\nline three".into();
        assert_eq!(body_content_height(&three, 80), 7);
    }

    #[test]
    fn comment_height_counts_header_body_footer_blank() {
        // header rule + 1 body line + footer rule + trailing blank.
        assert_eq!(comment_height(&test_comment("one line"), 80), 4);
        // header + 3 body + footer + blank.
        assert_eq!(comment_height(&test_comment("a\nb\nc"), 80), 6);
    }

    /// A table breaks the renderer's one-line-per-source-line property, so the
    /// only thing keeping the scroll clamps honest is that the measurement and
    /// the draw render through the same function at the same width.
    #[test]
    fn table_measured_height_matches_the_rendered_rows() {
        let body =
            "| Repo | Notes |\n|------|-------|\n| homelabia | needs a regression test today |";
        let mut with_table = issue(vec![]);
        with_table.body = body.into();

        for width in [30u16, 46, 80] {
            let lines = body_lines(&with_table, false, width as usize, &Theme::default());
            let drawn = linkmap::wrap(&lines, &[], width as usize).0.len();
            assert_eq!(
                body_content_height(&with_table, width) as usize,
                drawn,
                "at width {width}"
            );
        }
    }

    /// A fence also breaks the one-line-per-source-line property (delimiters
    /// dropped, content hard-broken), so it needs the same measured-vs-drawn
    /// agreement check as the table above — including a width narrow enough to
    /// force the fence renderer's own hard-break.
    #[test]
    fn fence_measured_height_matches_the_rendered_rows() {
        let body = "```rust\nfn very_long_function_name_that_will_need_to_wrap(a, b, c) {}\n```";
        let mut with_fence = issue(vec![]);
        with_fence.body = body.into();

        for width in [20u16, 46, 80] {
            let lines = body_lines(&with_fence, false, width as usize, &Theme::default());
            let drawn = linkmap::wrap(&lines, &[], width as usize).0.len();
            assert_eq!(
                body_content_height(&with_fence, width) as usize,
                drawn,
                "at width {width}"
            );
        }
    }

    /// Guards the test above against passing trivially: a renderer that left the
    /// table as raw pipes would also wrap, so pin the height the *table* layout
    /// produces (4 metadata + header + rule + a body row wrapped over 3 rows).
    #[test]
    fn table_body_expands_to_its_laid_out_row_count() {
        let body =
            "| Repo | Notes |\n|------|-------|\n| homelabia | needs a regression test today |";
        let mut with_table = issue(vec![]);
        with_table.body = body.into();
        // Measured against an empty body so the metadata header's own wrapping
        // at this width cancels out: the table contributes a header row, a rule,
        // and a body row laid out over three rows. Left as raw pipes it would
        // contribute four.
        let baseline = body_content_height(&issue(vec![]), 30);
        assert_eq!(body_content_height(&with_table, 30), baseline + 5);

        let lines = body_lines(&with_table, false, 30, &Theme::default());
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            rendered.iter().any(|l: &String| l.contains('┼')),
            "expected a table rule, got {rendered:?}"
        );
    }

    #[test]
    fn comment_height_accounts_for_wrapping() {
        // A body line far wider than the pane wraps into multiple visual rows,
        // so the measured height exceeds the naive source-line count.
        let long = "x".repeat(200);
        let h = comment_height(&test_comment(&long), 40);
        assert!(h > 4, "expected wrapped height > 4, got {h}");
    }

    #[test]
    fn comment_offset_sums_preceding_card_heights() {
        let comments = vec![test_comment("a\nb\nc"), test_comment("only one")];
        assert_eq!(comment_offset(&comments, 0, 80), 0);
        // First card is header + 3 body + footer + blank = 6 rows.
        assert_eq!(comment_offset(&comments, 1, 80), 6);
    }

    /// A detail-pane app with a long body and one long + two short comments,
    /// rendered into a `TestBackend` so the two-region layout can be asserted.
    fn detail_render_string(sel: DetailSel) -> String {
        use crate::provider::types::RepoIssues;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut i = issue(vec![]);
        i.number = 42;
        i.title = "Redesign the detail pane".into();
        i.body = (1..=20)
            .map(|n| format!("Body line {n} with enough words to possibly wrap in a narrow pane."))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.state_filter = crate::tui::app::StateFilter::All;
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![i],
        }]);
        app.selected = 1;
        app.open_detail();
        app.detail.comments = Some(vec![
            test_comment(
                &(1..=15)
                    .map(|n| format!("Comment line {n} long enough to scroll within one card."))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            test_comment("Second comment, short."),
            test_comment("Third comment."),
        ]);
        app.detail.sel = sel;
        if let DetailSel::Comment(idx) = sel {
            let w = layout::detail_inner_width(100);
            app.detail.comments_scroll =
                comment_offset(app.detail.comments.as_ref().unwrap(), idx, w);
        }

        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app, &Theme::default())).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn detail_pane_shows_two_regions_body_and_comments() {
        let out = detail_render_string(DetailSel::Comment(0));
        // Both region blocks are titled and present.
        assert!(out.contains("issue (Tab comment"), "body title missing");
        assert!(out.contains(" comments "), "comments title missing");
        // The pinned body metadata and description render in the top region.
        assert!(out.contains("#42"), "issue number missing");
        assert!(out.contains("Body line 1"), "body text missing");
        // The selected comment's header rule renders in the bottom region.
        assert!(out.contains("── octocat"), "comment header missing");
    }

    #[test]
    fn detail_pane_draws_scrollbars_when_content_overflows() {
        // Long body + long selected comment both overflow their regions, so a
        // scrollbar thumb (█) is drawn for each.
        let out = detail_render_string(DetailSel::Comment(0));
        assert!(out.contains('█'), "expected scrollbar thumbs to be drawn");
    }

    /// Render a detail pane whose body and first comment each hold a URL, then
    /// return every buffer cell symbol joined into one string (so the embedded
    /// OSC 8 escape sequences are visible).
    fn detail_hyperlink_symbols() -> String {
        use crate::provider::types::RepoIssues;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut i = issue(vec![]);
        i.body = "Visit https://example.com for the docs.".into();
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.state_filter = crate::tui::app::StateFilter::All;
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![i],
        }]);
        app.selected = 1;
        app.open_detail();
        app.detail.comments = Some(vec![test_comment(
            "See [the site](https://comment.example.org)",
        )]);

        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app, &Theme::default())).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn detail_urls_are_wrapped_in_osc8_hyperlinks() {
        let out = detail_hyperlink_symbols();
        // The body's bare URL is bracketed by an OSC 8 open (…;URL\e\\) and close.
        assert!(
            out.contains(";https://example.com\x1b\\"),
            "body URL not opened as a hyperlink"
        );
        // The markdown link's label is hyperlinked to its target URL.
        assert!(
            out.contains(";https://comment.example.org\x1b\\"),
            "comment link target not opened as a hyperlink"
        );
        // The closing sequence is present.
        assert!(out.contains("\x1b]8;;\x1b\\"), "no OSC 8 close sequence");
        // The visible URL text is preserved (the escapes only bracket it).
        assert!(out.contains("https://example.com"), "URL text was lost");
    }

    /// The drawn detail-pane geometry at several terminal sizes. Expected
    /// values are written out by hand rather than recomputed from the
    /// layout constants, so a change to those constants shows up here.
    #[test]
    fn golden_detail_pane_geometry() {
        // (cols, rows, expected pane x, expected body region height)
        //
        // Pane x: the list takes 40% of the full width, the detail pane the
        // rest. Body height: main area is rows - 2 (info + status lines),
        // split 45/55 with a 6-row floor on the body.
        for (cols, rows, want_x, want_body_h) in [
            (80u16, 24u16, 32u16, 9u16), // main 22 → body 45% = 9
            (100, 32, 40, 13),           // main 30 → body 45% = 13
            (200, 60, 80, 26),           // main 58 → body 45% = 26
        ] {
            let buf = render_app(&detail_app(), cols, rows);
            let pane_x = detail_pane_x(&buf);
            assert_eq!(pane_x, want_x, "detail pane x at {cols}x{rows}");

            let split_y = detail_region_split_y(&buf, pane_x);
            // Body region spans rows 0..=split_y, so its height is split_y + 1.
            assert_eq!(
                split_y + 1,
                want_body_h,
                "body region height at {cols}x{rows}"
            );

            // The comments region fills the rest of the main area.
            let main_h = rows - 2;
            assert_eq!(
                main_h - want_body_h,
                layout::detail_split(main_h).1,
                "comments region height at {cols}x{rows}"
            );
        }
    }

    /// The width the key handler wraps and clamps against must be the width
    /// actually drawn — `layout::detail_inner_width` is what
    /// `event::detail_metrics` feeds into every scroll clamp.
    #[test]
    fn golden_detail_pane_width_matches_drawn_pane() {
        for (cols, rows) in [(80u16, 24u16), (100, 32), (200, 60)] {
            let buf = render_app(&detail_app(), cols, rows);
            let pane_x = detail_pane_x(&buf);
            let drawn_inner = cols - pane_x - 2; // minus both border columns
            assert_eq!(
                layout::detail_inner_width(cols),
                drawn_inner,
                "detail_inner_width disagrees with the drawn pane at {cols}x{rows}"
            );
        }
    }
}
