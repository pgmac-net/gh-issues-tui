use std::num::NonZeroU16;

use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use unicode_width::UnicodeWidthStr;

use super::linkmap::{self, LinkRect};
use super::markdown::LinkSpan;
use crate::provider::types::{Comment, Issue, PrState, PrSummary, WorkflowRunInfo};

use super::app::{
    App, CommentFocus, ConfirmChoice, DetailSel, FILTER_FIELDS, Focus, INPUT_POPUP_WIDTH,
    ISSUE_FORM_CANCEL_ROW, ISSUE_FORM_CREATE_ROW, ISSUE_FORM_DESC_HEIGHT, ISSUE_FORM_FIELDS,
    ISSUE_FORM_LABEL_WIDTH, ISSUE_FORM_WIDTH, InputKind, IssueForm, Mode, PrTarget, Row,
    cursor_row, input_popup_width, input_scroll_skip, issue_form_width, wrap_lines,
};
use super::layout;
use super::markdown;
use super::theme::Theme;

pub fn draw(f: &mut Frame, app: &App, t: &Theme) {
    let frame = layout::frame(f.area());
    let panes = layout::panes(frame.main, app.detail_open);

    draw_list(f, app, t, panes.list);
    if let Some(detail) = panes.detail {
        draw_detail(f, app, t, detail);
    }
    draw_info_bar(f, app, t, frame.info);
    draw_bottom_line(f, app, t, frame.bottom);

    match app.mode {
        Mode::FilterMenu => draw_filter_menu(f, app, t),
        Mode::SelectField(idx) => draw_picker(f, app, t, PickerSpec::filter_field(idx, false)),
        Mode::SelectFieldMulti(idx) => draw_picker(f, app, t, PickerSpec::filter_field(idx, true)),
        Mode::Calendar(idx) => draw_calendar_popup(f, app, t, idx),
        Mode::IssueForm => draw_issue_form(f, app, t),
        Mode::IssueFormSelect(idx) => {
            draw_issue_form(f, app, t);
            draw_picker(f, app, t, PickerSpec::form_field(idx, false));
        }
        Mode::IssueFormMulti(idx) => {
            draw_issue_form(f, app, t);
            draw_picker(f, app, t, PickerSpec::form_field(idx, true));
        }
        Mode::Input(kind) => draw_input_popup(f, app, t, kind),
        Mode::PrioritySet => draw_picker(f, app, t, PickerSpec::priority()),
        Mode::LabelsSet => draw_picker(f, app, t, PickerSpec::labels()),
        Mode::PrPicker => draw_picker(f, app, t, PickerSpec::pr_links()),
        Mode::PrSummary => draw_pr_summary_popup(f, app, t),
        Mode::ConfirmState => draw_confirm_popup(f, app, t),
        Mode::Help => draw_help(f, t),
        _ => {}
    }
}

fn draw_list(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| match row {
            Row::RepoHeader { repo_idx } => {
                let repo = &app.repos[*repo_idx];
                let arrow = if app.collapsed.contains(&repo.repo) {
                    "▸"
                } else {
                    "▾"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{arrow} {}", repo.repo),
                        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ({})", app.repo_visible_count(*repo_idx)),
                        Style::default().fg(t.dim),
                    ),
                ]))
            }
            Row::Issue {
                repo_idx,
                issue_idx,
            } => issue_item(&app.repos[*repo_idx].issues[*issue_idx], t),
        })
        .collect();

    let title = if app.loading {
        format!(" {} — loading… ", app.org)
    } else {
        format!(" {} — {} issues ", app.org, app.filtered_issue_count())
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pane_border(app, t, Focus::List))
                .title(title),
        )
        .highlight_style(Style::default().bg(t.selected_bg))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    if !app.rows.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn issue_item(issue: &Issue, t: &Theme) -> ListItem<'static> {
    let state_span = match issue.state {
        crate::provider::types::IssueState::Open => Span::styled("●", Style::default().fg(t.open)),
        crate::provider::types::IssueState::Closed => {
            Span::styled("●", Style::default().fg(t.closed))
        }
    };
    let mut spans = vec![
        Span::raw("   "),
        state_span,
        Span::styled(format!(" #{:<5}", issue.number), Style::default().fg(t.dim)),
        Span::styled(issue.title.clone(), title_style(issue, t)),
    ];
    if !issue.assignees.is_empty() {
        spans.push(Span::styled(
            format!("  @{}", issue.assignees.join(",@")),
            Style::default().fg(t.assignee),
        ));
    }
    for label in &issue.labels {
        spans.push(Span::styled(
            format!(" [{}]", label.name),
            Style::default().fg(label_color(&label.color, t.label_fallback)),
        ));
    }
    if issue.comment_count > 0 {
        spans.push(Span::styled(
            format!(" 🗨{}", issue.comment_count),
            Style::default().fg(t.dim),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", issue.updated_at.format("%Y-%m-%d")),
        Style::default().fg(t.dim),
    ));
    ListItem::new(Line::from(spans))
}

/// Border style for a pane: accent when it has focus and the split is open.
fn pane_border(app: &App, t: &Theme, pane: Focus) -> Style {
    if app.detail_open && app.focus == pane {
        Style::default().fg(t.accent)
    } else {
        Style::default()
    }
}

fn draw_detail(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
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
fn draw_detail_body(f: &mut Frame, app: &App, t: &Theme, issue: &Issue, area: Rect, focused: bool) {
    let selected = focused && app.detail_sel == DetailSel::Body;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(" issue (Tab comment · j/k scroll · e edit · P PR · ← list · Esc close) ");
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    let (lines, links) = body_lines_links(issue, selected, t);
    let (wrapped, rects) = linkmap::wrap(&lines, &links, inner_w as usize);
    let content_h = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);
    let max_scroll = content_h.saturating_sub(inner_h);
    let scroll = app.body_scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(wrapped).block(block).scroll((scroll, 0)),
        area,
    );
    render_region_scrollbar(f, t, area, content_h, inner_h, scroll);
    apply_hyperlinks(f.buffer_mut(), inner_area(area), &rects, scroll);
}

/// The bottom region: the stacked comment cards, scrolled by `comments_scroll`,
/// with a scrollbar reflecting position within the *selected* comment.
fn draw_detail_comments(f: &mut Frame, app: &App, t: &Theme, area: Rect, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(" comments ");
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    let selected = match app.detail_sel {
        _ if !focused => None,
        DetailSel::Comment(i) => Some(i),
        DetailSel::Body => None,
    };

    let comments = match &app.detail_comments {
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

    let scroll = app.comments_scroll;
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

/// The content rectangle inside a bordered pane (one cell of border on each
/// side). Used to place OSC 8 hyperlinks after a `Paragraph` has drawn.
fn inner_area(area: Rect) -> Rect {
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
fn apply_hyperlinks(buf: &mut Buffer, inner: Rect, rects: &[LinkRect], scroll: u16) {
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
fn render_region_scrollbar(
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
/// and `body_content_height` so measured and drawn heights match.
fn body_lines(issue: &Issue, selected: bool, t: &Theme) -> Vec<Line<'static>> {
    body_lines_links(issue, selected, t).0
}

/// [`body_lines`] plus the URL positions in the description, with each link's
/// line index offset past the metadata header so it points into the returned
/// `Vec<Line>`.
fn body_lines_links(
    issue: &Issue,
    selected: bool,
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
    let (md_lines, md_links) = markdown::render_with_links(&issue.body, t);
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
fn comment_card_lines(
    c: &Comment,
    selected: bool,
    card_width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    comment_card_lines_links(c, selected, card_width, t).0
}

/// [`comment_card_lines`] plus the URL positions in the comment body, with each
/// link's line index offset past the header rule.
fn comment_card_lines_links(
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
    let (md_lines, md_links) = markdown::render_with_links(&c.body, t);
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

/// Wrapped (visual) height of `lines` at inner width `width`, via the same
/// [`linkmap`] wrapper the detail regions render with, so measured and drawn
/// heights match exactly.
fn paragraph_height(lines: &[Line<'static>], width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    linkmap::wrapped_height(lines, width as usize)
}

/// Wrapped height of the body region's content (metadata + description) at
/// inner width `width`. Styling doesn't affect wrapping, so a default theme
/// gives an exact count for the key handler's scroll clamp.
pub fn body_content_height(issue: &Issue, width: u16) -> u16 {
    paragraph_height(&body_lines(issue, false, &Theme::default()), width)
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
fn rule_line(prefix: &str, width: usize, style: Style) -> Line<'static> {
    let fill = width.saturating_sub(prefix.chars().count());
    Span::styled(format!("{prefix}{}", "─".repeat(fill)), style).into()
}

/// Title colour: the priority label's own colour when one is set, default otherwise.
fn title_style(issue: &Issue, t: &Theme) -> Style {
    match issue.priority_label() {
        Some(l) => Style::default().fg(label_color(&l.color, t.label_fallback)),
        None => Style::default(),
    }
}

fn state_style(issue: &Issue, t: &Theme) -> Style {
    match issue.state {
        crate::provider::types::IssueState::Open => Style::default().fg(t.open),
        crate::provider::types::IssueState::Closed => Style::default().fg(t.closed),
    }
}

fn draw_info_bar(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let mut spans = vec![
        Span::styled(" state:", Style::default().fg(t.dim)),
        Span::raw(app.state_filter.label()),
        Span::styled("  sort:", Style::default().fg(t.dim)),
        Span::raw(format!(
            "{}{}",
            app.sort_key.label(),
            if app.sort_desc { "↓" } else { "↑" }
        )),
    ];
    // Rate limit indicator
    if let Some(rl) = &app.rate_limit {
        let color = if rl.remaining < 10 {
            t.error
        } else if rl.remaining < 100 {
            t.warning
        } else {
            t.dim
        };
        spans.push(Span::styled(
            format!("  API {}/{}", rl.remaining, rl.limit),
            Style::default().fg(color),
        ));
    }
    if let Some(err) = &app.rate_limit_error {
        spans.push(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(t.error),
        ));
    } else if app.filters_active() {
        spans.push(Span::styled(
            "  [filters active — F to edit, F→c to clear]",
            Style::default().fg(t.warning),
        ));
    }
    spans.push(Span::styled("  ?:help", Style::default().fg(t.dim)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The prompt title shown on the single-line input popup for each kind.
fn input_prompt(kind: InputKind) -> &'static str {
    match kind {
        InputKind::Search => "search",
        InputKind::FilterField(idx) => FILTER_FIELDS[idx],
        InputKind::Assignees => "assignees (comma-separated logins)",
        InputKind::Title => "title",
        InputKind::Org => "org/owner (Enter switches)",
        InputKind::GotoNumber => "issue # (Enter jumps)",
    }
}

fn draw_bottom_line(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let msg = app.status.clone().unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {msg}"), Style::default().fg(t.dim))),
        area,
    );
}

/// Centered close/reopen confirmation popup (`Mode::ConfirmState`), with a
/// `[ Yes ]  [ No ]` button row reusing the reversed-video focused-button
/// style from the inline comment editor.
fn draw_confirm_popup(f: &mut Frame, app: &App, t: &Theme) {
    let action = app
        .selected_issue()
        .map(|i| match i.state {
            crate::provider::types::IssueState::Open => "close",
            crate::provider::types::IssueState::Closed => "reopen",
        })
        .unwrap_or("toggle");
    let message = match app.selected_issue() {
        Some(i) => format!("{action} issue #{}?", i.number),
        None => format!("{action} this issue?"),
    };

    let width = (message.len() as u16 + 4).max(24);
    let area = centered(f.area(), width, 4);
    f.render_widget(Clear, area);

    const YES: &str = "[ Yes ]";
    const NO: &str = "[ No ]";
    let gap = "  ";
    let inner_width = area.width.saturating_sub(2) as usize;
    let total = YES.len() + gap.len() + NO.len();
    let pad = " ".repeat(inner_width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    let buttons = Line::from(vec![
        Span::raw(pad),
        Span::styled(YES, button_style(app.confirm_choice == ConfirmChoice::Yes)),
        Span::raw(gap),
        Span::styled(NO, button_style(app.confirm_choice == ConfirmChoice::No)),
    ]);

    let para = Paragraph::new(vec![
        Line::styled(message, Style::default().fg(t.warning)),
        buttons,
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {action} issue ")),
    );
    f.render_widget(para, area);
}

fn draw_filter_menu(f: &mut Frame, app: &App, t: &Theme) {
    let area = centered(f.area(), 60, FILTER_FIELDS.len() as u16 + 4);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = FILTER_FIELDS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let value = app.current_filter_value(i);
            let style = if i == app.filter_menu_idx {
                Style::default().bg(t.selected_bg)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<28}"), style.fg(t.accent)),
                Span::styled(value, style),
            ]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" filters (Enter edit · c clear all · Esc close) "),
    );
    f.render_widget(list, area);
}

/// Rows for an option picker under the active type-ahead filter: a `/`
/// row while a filter is typed, then the filtered options. The highlight
/// is positional within the filtered view; multi-select `[x]` marks and
/// the "—" clear row key off original option indices. ASCII prefix on
/// purpose — emoji cell widths are unreliable across terminals.
fn picker_items(app: &App, t: &Theme, multi: bool, clear_label: &str) -> Vec<ListItem<'static>> {
    let mut items: Vec<ListItem> = Vec::new();
    if !app.select_filter.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(" / ", Style::default().fg(t.accent)),
            Span::raw(app.select_filter.clone()),
            Span::styled("█", Style::default().fg(t.accent)),
        ])));
    }
    let filtered = app.filtered_select();
    if filtered.is_empty() {
        let msg = if app.select_options.is_empty() {
            " nothing available"
        } else {
            " no matches"
        };
        items.push(ListItem::new(Line::styled(
            msg.to_string(),
            Style::default().fg(t.dim),
        )));
        return items;
    }
    for (pos, (orig, opt)) in filtered.into_iter().enumerate() {
        let style = if pos == app.select_idx {
            Style::default().bg(t.selected_bg)
        } else {
            Style::default()
        };
        let text = if multi {
            let mark = if app.multi_selected.contains(&orig) {
                "[x]"
            } else {
                "[ ]"
            };
            format!(" {mark} {opt}")
        } else if opt == "\u{2014}" {
            format!(" \u{2014} {clear_label} \u{2014}")
        } else {
            format!(" {opt}")
        };
        items.push(ListItem::new(Line::from(Span::styled(text, style))));
    }
    items
}

/// Popup height for `rows` list items (+2 borders), clamped to the frame.
fn picker_height(f: &Frame, rows: usize) -> u16 {
    (rows.max(1) as u16 + 2).min(f.area().height)
}

/// Default popup width for every picker but the PR one, which needs more
/// room for `owner/repo#number` entries.
const PICKER_WIDTH: u16 = 50;
const PR_PICKER_WIDTH: u16 = 60;

/// What distinguishes one picker popup from another. Everything else — the
/// item list, the centring, the border — is identical across all of them.
struct PickerSpec {
    /// Full border title, including its surrounding spaces. Longer than the
    /// popup is wide in most cases; the border clips it.
    title: String,
    width: u16,
    /// Space toggles and items carry `[ ]`/`[x]` marks.
    multi: bool,
    /// Wording for the leading "clear this field" entry.
    clear_label: &'static str,
}

impl PickerSpec {
    /// A standard-width picker clearing to "clear".
    fn new(title: impl Into<String>, multi: bool) -> Self {
        Self {
            title: title.into(),
            width: PICKER_WIDTH,
            multi,
            clear_label: "clear",
        }
    }

    /// Picker for a filter field (`Mode::SelectField` / `SelectFieldMulti`).
    fn filter_field(idx: usize, multi: bool) -> Self {
        let field_name = FILTER_FIELDS[idx];
        let hint = if multi {
            "type filters · Space toggles · Enter accepts"
        } else {
            "type to filter · Enter picks · Esc cancels"
        };
        Self::new(format!(" select {field_name} ({hint}) "), multi)
    }

    /// Picker for a new-issue form field (`Mode::IssueFormSelect` /
    /// `IssueFormMulti`). These clear to "none" rather than "clear".
    fn form_field(idx: usize, multi: bool) -> Self {
        let field_name = ISSUE_FORM_FIELDS[idx];
        let hint = if multi {
            "type filters · Space toggles · Enter accepts"
        } else {
            "type filters · Enter picks · Esc cancels"
        };
        Self {
            clear_label: "none",
            ..Self::new(format!(" {field_name} ({hint}) "), multi)
        }
    }

    /// Setting the selected issue's priority (`Mode::PrioritySet`).
    fn priority() -> Self {
        Self::new(
            " set priority (type to filter · Enter sets · Esc cancels) ",
            false,
        )
    }

    /// Editing the selected issue's labels (`Mode::LabelsSet`).
    fn labels() -> Self {
        Self::new(
            " set labels (type to filter · Space toggles · Enter accepts · Esc cancels) ",
            true,
        )
    }

    /// Choosing among several linked PRs (`Mode::PrPicker`).
    fn pr_links() -> Self {
        Self {
            width: PR_PICKER_WIDTH,
            ..Self::new(
                " linked PRs (type to filter · Enter picks · Esc cancels) ",
                false,
            )
        }
    }
}

/// The one picker popup. Every `Mode` that shows a list of choices renders
/// through here; only the [`PickerSpec`] differs.
fn draw_picker(f: &mut Frame, app: &App, t: &Theme, spec: PickerSpec) {
    let items = picker_items(app, t, spec.multi, spec.clear_label);
    let area = centered(f.area(), spec.width, picker_height(f, items.len()));
    f.render_widget(Clear, area);
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(spec.title));
    f.render_widget(list, area);
}

/// Symbol + colour for a GitHub check/status conclusion string.
fn conclusion_style(conclusion: Option<&str>, t: &Theme) -> (&'static str, Color) {
    match conclusion.unwrap_or("PENDING") {
        "SUCCESS" => ("✔", t.open),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" => ("✘", t.error),
        "CANCELLED" | "SKIPPED" | "NEUTRAL" | "STALE" => ("-", t.dim),
        _ => ("…", t.warning),
    }
}

/// Outer width of the PR summary popup, before its borders.
const PR_SUMMARY_WIDTH: u16 = 76;

/// The PR summary popup's inner text width for a frame `frame_width` wide.
/// Shared by the renderer and the key handler so both measure the same rows.
pub fn pr_summary_inner_width(frame_width: u16) -> u16 {
    PR_SUMMARY_WIDTH.min(frame_width).saturating_sub(2)
}

/// The PR summary popup's outer area within `frame`.
fn pr_summary_area(frame: Rect) -> Rect {
    centered(frame, PR_SUMMARY_WIDTH, (frame.height * 3 / 4).max(12))
}

/// One drawn row of the PR summary popup: the line as rendered, plus the URL
/// it opens when it is a navigable row.
///
/// This is the popup's single source of truth. The renderer draws
/// `rows[i].line`; [`pr_targets`] reports position `i` for every row carrying
/// a URL. The two cannot disagree, because there is only one sequence.
pub struct PrRow {
    pub line: Line<'static>,
    pub url: Option<String>,
}

/// Build the popup's rows, already wrapped to `width`.
///
/// Wrapping happens here rather than in the `Paragraph` so that a row index
/// means the same thing to the renderer and to the key handler — the house
/// rule the detail pane already follows (see [`super::linkmap`]). A logical
/// line that wraps contributes several rows; only its first carries the URL,
/// so selecting it scrolls to where the item starts.
pub fn pr_summary_rows(
    summary: Option<&Result<PrSummary, String>>,
    t: &Theme,
    width: u16,
) -> Vec<PrRow> {
    let tagged = pr_summary_logical_rows(summary, t);
    let mut out = Vec::new();
    for (line, url) in tagged {
        let (wrapped, _) = linkmap::wrap(&[line], &[], width as usize);
        for (i, line) in wrapped.into_iter().enumerate() {
            out.push(PrRow {
                line,
                // Only the first wrapped row of an item is its target.
                url: if i == 0 { url.clone() } else { None },
            });
        }
    }
    out
}

/// The navigable rows of the PR summary popup, as positions in
/// [`pr_summary_rows`]' output. Derived, never separately computed.
pub fn pr_targets(summary: Option<&Result<PrSummary, String>>, width: u16) -> Vec<PrTarget> {
    // Styling does not affect row count or URLs, so the default theme is a
    // sound basis for measurement — the same convention `body_content_height`
    // uses.
    pr_summary_rows(summary, &Theme::default(), width)
        .into_iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.url.map(|url| PrTarget {
                url,
                line: i as u16,
            })
        })
        .collect()
}

/// The popup's unwrapped lines, each tagged with the URL it opens (if any).
fn pr_summary_logical_rows(
    summary: Option<&Result<PrSummary, String>>,
    t: &Theme,
) -> Vec<(Line<'static>, Option<String>)> {
    let plain = |line: Line<'static>| (line, None);

    let s = match summary {
        None => {
            return vec![plain(Line::styled(
                "loading PR summary…",
                Style::default().fg(t.dim),
            ))];
        }
        Some(Err(e)) => {
            return vec![plain(Line::styled(
                format!("failed: {e}"),
                Style::default().fg(t.error),
            ))];
        }
        Some(Ok(s)) => s,
    };

    let mut rows: Vec<(Line<'static>, Option<String>)> = Vec::new();

    // The PR header itself is the first navigable row.
    rows.push((
        Line::from(vec![
            Span::styled(format!("{} ", s.pr.label()), Style::default().fg(t.dim)),
            Span::styled(
                s.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Some(s.pr.url()),
    ));
    rows.push(plain(Line::from(vec![
        Span::styled(
            if s.is_draft {
                "draft ".to_string()
            } else {
                format!("{} ", s.state)
            },
            Style::default().fg(match s.state {
                PrState::Merged => t.assignee,
                PrState::Open => t.open,
                PrState::Closed => t.closed,
            }),
        ),
        Span::styled(
            format!(
                "{} ← {}   +{}/-{} · {} files",
                s.base_ref, s.head_ref, s.additions, s.deletions, s.changed_files
            ),
            Style::default().fg(t.dim),
        ),
    ])));
    rows.push(plain(Line::default()));

    for l in s.body.lines() {
        rows.push(plain(Line::raw(l.to_string())));
    }
    rows.push(plain(Line::default()));

    let review_line = match s.reviews.decision {
        Some(d) => format!("{d}"),
        None => "no reviews yet".to_string(),
    };
    rows.push(plain(Line::from(vec![
        Span::styled("reviews: ", Style::default().fg(t.accent)),
        Span::raw(format!(
            "{review_line} · {} approved, {} changes requested, {} commented",
            s.reviews.approved, s.reviews.changes_requested, s.reviews.commented
        )),
    ])));
    rows.push(plain(Line::from(vec![
        Span::styled("comments: ", Style::default().fg(t.accent)),
        Span::raw(format!(
            "{} · {} review threads",
            s.comment_count, s.review_thread_count
        )),
    ])));
    rows.push(plain(Line::default()));

    rows.push(plain(Line::from(vec![
        Span::styled("checks: ", Style::default().fg(t.accent)),
        Span::raw(s.checks.state.clone().unwrap_or_else(|| "none".into())),
    ])));
    for c in &s.checks.contexts {
        let (sym, color) = conclusion_style(Some(c.conclusion.as_str()), t);
        rows.push((
            Line::from(vec![
                Span::styled(format!("  {sym} "), Style::default().fg(color)),
                Span::raw(c.name.clone()),
            ]),
            Some(c.url.clone()),
        ));
    }

    let mut run_section = |heading: Line<'static>, runs: &[WorkflowRunInfo]| {
        if runs.is_empty() {
            return;
        }
        rows.push(plain(Line::default()));
        rows.push(plain(heading));
        for r in runs {
            let (sym, color) = conclusion_style(r.conclusion.as_deref(), t);
            rows.push((
                Line::from(vec![
                    Span::styled(format!("  {sym} "), Style::default().fg(color)),
                    Span::raw(format!("{} #{} ({})", r.workflow, r.run_number, r.event)),
                ]),
                Some(r.url.clone()),
            ));
        }
    };

    run_section(
        Line::styled("PR workflow runs:", Style::default().fg(t.accent)),
        &s.pr_runs,
    );
    run_section(
        Line::styled(
            format!("── default branch ({}) ──", s.default_branch_name),
            Style::default().fg(t.accent),
        ),
        &s.default_branch_runs,
    );

    rows
}

fn draw_pr_summary_popup(f: &mut Frame, app: &App, t: &Theme) {
    let area = pr_summary_area(f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(" PR summary (j/k scroll · Tab select · o open · r refresh · Esc close) ");

    let width = pr_summary_inner_width(f.area().width);
    let rows = pr_summary_rows(app.pr_summary.as_ref(), t, width);
    let mut lines: Vec<Line> = rows.into_iter().map(|r| r.line).collect();

    // Highlight the selected row (`Tab`/`Shift+Tab`) by patching a
    // background onto each of its spans' existing styles, preserving their
    // foreground colours and modifiers.
    if let Some(sel_line) = pr_targets(app.pr_summary.as_ref(), width)
        .get(app.pr_sel)
        .map(|target| target.line as usize)
        && let Some(l) = lines.get_mut(sel_line)
    {
        let old = std::mem::take(l);
        *l = Line::from(
            old.spans
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(t.selected_bg)))
                .collect::<Vec<_>>(),
        );
    }

    // Wrapping is already applied by `pr_summary_rows`, so the Paragraph must
    // not wrap again — otherwise a drawn row would stop matching its index.
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((app.pr_scroll, 0));
    f.render_widget(para, area);
}

/// The single inline new-issue form: title and description edit in place,
/// choice fields show their current selection and open a picker popup on
/// Enter, `[ Create ]`/`[ Cancel ]` sit at the bottom. `Tab`/`Shift+Tab`
/// move focus; the focused row is highlighted (a cursor for text fields,
/// a background fill for choice fields and the buttons).
fn draw_issue_form(f: &mut Frame, app: &App, t: &Theme) {
    let Some(form) = &app.issue_form else { return };
    let value_width =
        (issue_form_width(f.area().width) as usize).saturating_sub(ISSUE_FORM_LABEL_WIDTH);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(form_title_line(form, t, value_width));
    lines.extend(form_description_lines(form, t, value_width));
    for (i, name) in ISSUE_FORM_FIELDS.iter().enumerate().skip(2) {
        let focused = form.field_idx == i;
        let style = if focused {
            Style::default().bg(t.selected_bg)
        } else {
            Style::default()
        };
        let value = if form.options.is_none() {
            "loading…".to_string()
        } else {
            form.field_display(i)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {name:<ISSUE_FORM_LABEL_WIDTH$}"),
                style.fg(t.accent),
            ),
            Span::styled(value, style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(issue_form_button_line(
        form,
        t,
        value_width + ISSUE_FORM_LABEL_WIDTH,
    ));

    let area = centered(
        f.area(),
        ISSUE_FORM_WIDTH,
        (lines.len() as u16 + 2).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(
                " new issue in {} (Tab move · Enter edit/activate · Esc cancel) ",
                form.repo
            )),
    );
    f.render_widget(para, area);
}

/// The title row: an inline single-line editor, horizontally scrolled to
/// keep the cursor visible when focused.
fn form_title_line(form: &IssueForm, t: &Theme, value_width: usize) -> Line<'static> {
    let focused = form.field_idx == 0;
    let style = if focused {
        Style::default().bg(t.selected_bg)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(
        format!(" {:<ISSUE_FORM_LABEL_WIDTH$}", ISSUE_FORM_FIELDS[0]),
        style.fg(t.accent),
    )];
    if focused {
        let skip = input_scroll_skip(form.title.cursor, value_width);
        let visible: String = form
            .title
            .buffer
            .chars()
            .skip(skip)
            .take(value_width)
            .collect();
        spans.extend(
            cursor_spans(&visible, form.title.cursor - skip)
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(t.selected_bg))),
        );
    } else {
        spans.push(Span::raw(form.title.buffer.clone()));
    }
    Line::from(spans)
}

/// The description row: a label line, then a fixed-height, word-wrapped
/// inline block that scrolls to keep the cursor's visual row visible when
/// focused (mirrors the inline comment editor, minus its own border).
fn form_description_lines(form: &IssueForm, t: &Theme, value_width: usize) -> Vec<Line<'static>> {
    let focused = form.field_idx == 1;
    let label_style = if focused {
        Style::default().bg(t.selected_bg)
    } else {
        Style::default()
    };
    let mut lines = vec![Line::from(Span::styled(
        format!(" {:<ISSUE_FORM_LABEL_WIDTH$}", ISSUE_FORM_FIELDS[1]),
        label_style.fg(t.accent),
    ))];

    let rows = wrap_lines(&form.body.lines, value_width);
    let (cur_row, cur_col) = cursor_row(
        &rows,
        form.body.line,
        form.body.lines[form.body.line].cursor,
    );
    let top = if focused {
        cur_row.saturating_sub(ISSUE_FORM_DESC_HEIGHT.saturating_sub(1))
    } else {
        0
    };
    for i in 0..ISSUE_FORM_DESC_HEIGHT {
        let row_idx = top + i;
        let text = rows
            .get(row_idx)
            .map(|row| {
                form.body.lines[row.line]
                    .buffer
                    .chars()
                    .skip(row.start)
                    .take(row.end - row.start)
                    .collect::<String>()
            })
            .unwrap_or_default();
        let mut spans = vec![Span::raw("   ")];
        if focused && row_idx == cur_row {
            spans.extend(cursor_spans(&text, cur_col));
        } else {
            spans.push(Span::raw(text));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// The centered `[ Create ]  [ Cancel ]` button row, with the focused
/// button drawn in reversed video.
fn issue_form_button_line(form: &IssueForm, t: &Theme, width: usize) -> Line<'static> {
    const CREATE: &str = "[ Create ]";
    const CANCEL: &str = "[ Cancel ]";
    let gap = "  ";
    let total = CREATE.len() + gap.len() + CANCEL.len();
    let pad = " ".repeat(width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    Line::from(vec![
        Span::raw(pad),
        Span::styled(
            CREATE,
            button_style(form.field_idx == ISSUE_FORM_CREATE_ROW),
        ),
        Span::raw(gap),
        Span::styled(
            CANCEL,
            button_style(form.field_idx == ISSUE_FORM_CANCEL_ROW),
        ),
    ])
}

/// Option popup for a form field: single-select (with the "—" clear row)
/// or multi-select (Space toggles, checkbox markers).
/// The inline comment section at the bottom of the detail pane
/// (`Mode::CommentEditor`): a multi-line editor with a `[ Save ]  [ Cancel ]`
/// button row, the focused element highlighted. Width matches
/// `comment_pane_width` so the renderer and the key handler's up/down
/// visual-row navigation agree on wrap geometry.
fn draw_comment_section(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let width = layout::detail_inner_width(f.area().width) as usize;
    // One row reserved for the button line at the bottom of the block.
    let inner_height = area.height.saturating_sub(2) as usize;
    let text_height = inner_height.saturating_sub(1);

    let body = &app.comment_editor;
    let rows = wrap_lines(&body.lines, width);
    let (cur_row, cur_col) = cursor_row(&rows, body.line, body.lines[body.line].cursor);
    let top = cur_row.saturating_sub(text_height.saturating_sub(1));
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(text_height)
        .map(|(i, row)| {
            let text: String = body.lines[row.line]
                .buffer
                .chars()
                .skip(row.start)
                .take(row.end - row.start)
                .collect();
            if i == cur_row && app.comment_focus == CommentFocus::Editor {
                Line::from(cursor_spans(&text, cur_col))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    lines.push(comment_button_line(app, t, width));

    let action = match app.editor_target {
        crate::tui::app::EditorTarget::NewComment => "add comment",
        crate::tui::app::EditorTarget::EditComment { .. } => "edit comment",
        crate::tui::app::EditorTarget::EditBody => "edit description",
    };
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(" {action} (Ctrl+S save · Esc cancel) ")),
    );
    f.render_widget(para, area);
}

/// The centered `[ Save ]  [ Cancel ]` button row, with the focused button
/// drawn in reversed video.
fn comment_button_line(app: &App, t: &Theme, width: usize) -> Line<'static> {
    const SAVE: &str = "[ Save ]";
    const CANCEL: &str = "[ Cancel ]";
    let gap = "  ";
    let total = SAVE.len() + gap.len() + CANCEL.len();
    let pad = " ".repeat(width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    Line::from(vec![
        Span::raw(pad),
        Span::styled(SAVE, button_style(app.comment_focus == CommentFocus::Save)),
        Span::raw(gap),
        Span::styled(
            CANCEL,
            button_style(app.comment_focus == CommentFocus::Cancel),
        ),
    ])
}

/// Single-line input popup used for search/filters/assignees/labels/title/
/// org/new-issue-title. Horizontally scrolls so the cursor always stays
/// visible when the value is wider than the box.
fn draw_input_popup(f: &mut Frame, app: &App, t: &Theme, kind: InputKind) {
    let area = centered(f.area(), INPUT_POPUP_WIDTH, 3.min(f.area().height));
    f.render_widget(Clear, area);
    let width = input_popup_width(f.area().width) as usize;

    let skip = input_scroll_skip(app.input.cursor, width);
    let visible: String = app.input.buffer.chars().skip(skip).take(width).collect();
    let col = app.input.cursor - skip;

    let para = Paragraph::new(Line::from(cursor_spans(&visible, col))).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(" {} ", input_prompt(kind))),
    );
    f.render_widget(para, area);
}

/// The text with the char at `cursor` drawn as a block cursor (reversed
/// video); a reversed space when the cursor sits past the end of the text.
fn cursor_spans(text: &str, cursor: usize) -> Vec<Span<'static>> {
    let byte = text
        .char_indices()
        .nth(cursor)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    let mut rest = text[byte..].chars();
    let under = rest.next().unwrap_or(' ').to_string();
    let after: String = rest.collect();
    vec![
        Span::raw(text[..byte].to_string()),
        Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]
}

fn draw_calendar_popup(f: &mut Frame, app: &App, t: &Theme, idx: usize) {
    let field_name = FILTER_FIELDS[idx];
    let cursor = app.calendar_cursor;

    let first = NaiveDate::from_ymd_opt(cursor.year(), cursor.month(), 1).unwrap();
    let next_first = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
    };
    let last = next_first.pred_opt().unwrap_or(next_first);
    let dow_offset = first.weekday().num_days_from_monday() as usize;
    let days_in_month = last.day();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("{} {}", cursor.format("%B"), cursor.year()),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(" Mo Tu We Th Fr Sa Su".to_string()));

    let mut day = 1u32;
    for _row in 0..6 {
        if day > days_in_month {
            break;
        }
        let mut week: Vec<Span> = Vec::new();
        for col in 0..7 {
            if day == 1 && col < dow_offset {
                week.push(Span::raw("   ".to_string()));
            } else if day <= days_in_month {
                let style = if day == cursor.day() {
                    Style::default()
                        .bg(t.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                week.push(Span::styled(format!("{:>2} ", day), style));
                day += 1;
            }
        }
        if !week.is_empty() {
            lines.push(Line::from(week));
        }
    }

    lines.push(Line::raw("".to_string()));
    lines.push(Line::from(vec![
        Span::styled("\u{2190}\u{2192} day  ", Style::default().fg(t.dim)),
        Span::styled("\u{2191}\u{2193} week  ", Style::default().fg(t.dim)),
        Span::styled("PgUp/PgDn month  ", Style::default().fg(t.dim)),
        Span::styled("Enter select  Esc cancel", Style::default().fg(t.dim)),
    ]));

    let height = lines.len() as u16 + 2;
    let area = centered(f.area(), 32, height);
    f.render_widget(Clear, area);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {field_name} ")),
    );
    f.render_widget(para, area);
}

fn draw_help(f: &mut Frame, t: &Theme) {
    const HELP: &[(&str, &str)] = &[
        ("j/k ↑/↓", "move list / scroll detail region"),
        ("Space", "collapse/expand repo group"),
        ("←", "collapse repo group / back to list"),
        ("→", "expand repo group / into detail pane"),
        ("[ / ]", "collapse all / expand all"),
        ("Enter", "open issue in detail pane"),
        (
            "Tab",
            "next comment in pane / switch pane (Shift+Tab reverse)",
        ),
        ("Esc / q", "close detail pane"),
        ("o / O", "open issue / repo in browser"),
        ("y", "copy issue ref to clipboard"),
        ("/", "text search"),
        ("#", "jump to issue number"),
        ("f", "cycle state filter (open/closed/all)"),
        ("F", "filter editor (pickers + calendar)"),
        ("s / S", "cycle sort key / toggle direction"),
        ("w", "switch org/owner"),
        ("c", "add comment (inline, Tab to buttons, Ctrl+S save)"),
        ("e", "edit description / comment (detail pane)"),
        ("x", "close / reopen issue"),
        ("a", "edit assignees"),
        ("l", "edit labels"),
        ("t", "edit title"),
        ("p", "set priority"),
        ("P", "summarise linked PR (in detail pane)"),
        ("n", "new issue"),
        ("r", "reload"),
        ("q", "back / quit"),
    ];
    let area = centered(f.area(), 52, HELP.len() as u16 + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), Style::default().fg(t.accent).bold()),
                Span::raw(*v),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" keys ")
                .title_bottom(
                    Line::from(format!(" v{} ", env!("CARGO_PKG_VERSION"))).right_aligned(),
                ),
        ),
        area,
    );
}

/// GitHub label colors arrive as 6-digit hex without `#`.
fn label_color(hex: &str, fallback: Color) -> Color {
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

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::{Issue, IssueState, Label};

    fn issue(labels: Vec<Label>) -> Issue {
        Issue {
            id: "id".into(),
            number: 114,
            title: "Upgrade Calico".into(),
            body: String::new(),
            state: IssueState::Open,
            url: String::new(),
            author: String::new(),
            assignees: vec![],
            labels,
            comment_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            closed_at: None,
        }
    }

    #[test]
    fn title_style_uses_priority_label_color() {
        let i = issue(vec![
            Label {
                name: "migrated-from-linear".into(),
                color: "ededed".into(),
            },
            Label {
                name: "priority:high".into(),
                color: "d93f0b".into(),
            },
        ]);
        let style = title_style(&i, &Theme::default());
        assert_eq!(style.fg, Some(Color::Rgb(0xd9, 0x3f, 0x0b)));
    }

    #[test]
    fn title_style_default_without_priority() {
        let i = issue(vec![Label {
            name: "bug".into(),
            color: "d73a4a".into(),
        }]);
        assert_eq!(title_style(&i, &Theme::default()).fg, None);
    }

    fn test_comment(body: &str) -> Comment {
        Comment {
            id: "c".into(),
            author: "octocat".into(),
            created_at: chrono::Utc::now(),
            body: body.into(),
        }
    }

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

    /// Single-repo app with one issue in `state`, selected, `Mode::ConfirmState`.
    fn confirm_app(state: IssueState) -> App {
        use crate::provider::types::RepoIssues;

        let mut i = issue(vec![]);
        i.state = state;
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
        app.selected = 1; // 0 = repo header, 1 = the issue
        app.mode = super::Mode::ConfirmState;
        app
    }

    fn render_confirm_buffer(app: &App) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_confirm_popup(f, app, &Theme::default()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rendered_confirm_popup(app: &App) -> String {
        render_confirm_buffer(app)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// True if the cell at the start of `needle`'s first match is drawn
    /// reversed-video (the focused-button style).
    fn is_reversed_at(buf: &ratatui::buffer::Buffer, needle: &str) -> bool {
        let width = buf.area().width;
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let byte_idx = content.find(needle).expect("needle not found in buffer");
        let cell_idx = content[..byte_idx].chars().count();
        let x = (cell_idx as u16) % width;
        let y = (cell_idx as u16) / width;
        buf[(x, y)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED)
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
        app.detail_comments = Some(vec![
            test_comment(
                &(1..=15)
                    .map(|n| format!("Comment line {n} long enough to scroll within one card."))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            test_comment("Second comment, short."),
            test_comment("Third comment."),
        ]);
        app.detail_sel = sel;
        if let DetailSel::Comment(idx) = sel {
            let w = layout::detail_inner_width(100);
            app.comments_scroll = comment_offset(app.detail_comments.as_ref().unwrap(), idx, w);
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
        app.detail_comments = Some(vec![test_comment(
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

    #[test]
    fn confirm_popup_prompts_close_for_open_issue() {
        let app = confirm_app(IssueState::Open);
        let text = rendered_confirm_popup(&app);
        assert!(text.contains("close issue"), "got: {text}");
        assert!(text.contains("#114"), "got: {text}");
        assert!(text.contains("Yes"), "got: {text}");
        assert!(text.contains("No"), "got: {text}");
    }

    #[test]
    fn confirm_popup_prompts_reopen_for_closed_issue() {
        let app = confirm_app(IssueState::Closed);
        let text = rendered_confirm_popup(&app);
        assert!(text.contains("reopen issue"), "got: {text}");
    }

    #[test]
    fn confirm_popup_highlights_focused_button() {
        let mut app = confirm_app(IssueState::Open);

        app.confirm_choice = super::ConfirmChoice::Yes;
        let buf = render_confirm_buffer(&app);
        assert!(is_reversed_at(&buf, "Yes"));
        assert!(!is_reversed_at(&buf, "No"));

        app.confirm_choice = super::ConfirmChoice::No;
        let buf = render_confirm_buffer(&app);
        assert!(!is_reversed_at(&buf, "Yes"));
        assert!(is_reversed_at(&buf, "No"));
    }

    #[test]
    fn help_popup_renders_version() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_help(f, &Theme::default())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains(concat!("v", env!("CARGO_PKG_VERSION"))),
            "version not found in help popup"
        );
    }

    // ----------------------------------------------------------------------
    // Characterisation goldens (issue #87).
    //
    // These pin what the screen actually looks like today, so the refactor
    // phases that follow have something to preserve. They read geometry back
    // out of the rendered buffer rather than recomputing it from the same
    // constants the renderer used — a test that repeats the production
    // arithmetic proves only that the arithmetic was copied correctly.
    //
    // Do not edit these to make a refactor pass. A golden that needs to
    // change means behaviour changed; investigate that first.
    // ----------------------------------------------------------------------

    fn test_app() -> App {
        App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        )
    }

    /// Render the whole UI — mode dispatch included — into a `TestBackend`.
    fn render_app(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app, &Theme::default())).unwrap();
        terminal.backend().buffer().clone()
    }

    /// A bordered box found in the rendered buffer, with its rows as strings.
    #[derive(Debug)]
    struct PopupBox {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        rows: Vec<String>,
    }

    impl PopupBox {
        /// The whole box as one string, for substring assertions.
        fn text(&self) -> String {
            self.rows.join("\n")
        }
    }

    fn cell_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> String {
        buf[(x, y)].symbol().to_string()
    }

    /// Locate the topmost overlay popup: the bordered box whose top-left
    /// corner sits furthest right. Overlays here are centred and narrower
    /// than whatever they cover, so the most-indented corner is the one
    /// drawn last. The list pane's own corner at x = 0 is never a candidate.
    fn popup_box(buf: &ratatui::buffer::Buffer) -> PopupBox {
        let area = *buf.area();
        let mut best: Option<(u16, u16)> = None;
        for y in 0..area.height {
            for x in 1..area.width {
                if cell_at(buf, x, y) == "┌" && best.is_none_or(|(bx, _)| x > bx) {
                    best = Some((x, y));
                }
            }
        }
        let (x, y) = best.expect("no popup box found in buffer");

        let mut width = 1;
        while x + width < area.width && cell_at(buf, x + width, y) != "┐" {
            width += 1;
        }
        width += 1; // include the closing corner

        let mut height = 1;
        while y + height < area.height && cell_at(buf, x, y + height) != "└" {
            height += 1;
        }
        height += 1; // include the bottom border row

        let rows = (y..y + height)
            .map(|row| (x..x + width).map(|col| cell_at(buf, col, row)).collect())
            .collect();

        PopupBox {
            x,
            y,
            width,
            height,
            rows,
        }
    }

    /// An app sitting in `mode` with a loaded picker: three options behind a
    /// leading clear entry, the second real option highlighted.
    fn picker_app(mode: Mode) -> App {
        let mut app = test_app();
        app.start_picker(
            vec!["—".into(), "alpha".into(), "beta".into(), "gamma".into()],
            2,
        );
        app.mode = mode;
        app
    }

    #[test]
    fn golden_filter_select_popup() {
        let app = picker_app(Mode::SelectField(1));
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "filter select popup width");
        let text = popup.text();
        // Field name comes from FILTER_FIELDS[1].
        assert!(
            text.contains("select repo (type to filter · Enter picks"),
            "title missing: {text}"
        );
        // Filter pickers label the clear entry "clear".
        assert!(text.contains("— clear —"), "clear entry missing: {text}");
        assert!(text.contains("alpha"), "options missing: {text}");
        // Single-select draws no checkbox marks.
        assert!(!text.contains("[ ]"), "unexpected multi marks: {text}");
    }

    #[test]
    fn golden_filter_select_multi_popup() {
        let mut app = picker_app(Mode::SelectFieldMulti(4));
        app.multi_selected.insert(2); // "beta"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "filter multi popup width");
        let text = popup.text();
        assert!(
            text.contains("select priority (type filters · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] beta"), "checked mark missing: {text}");
        assert!(text.contains("[ ] alpha"), "unchecked mark missing: {text}");
    }

    #[test]
    fn golden_priority_set_popup() {
        let app = picker_app(Mode::PrioritySet);
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "priority popup width");
        let text = popup.text();
        assert!(
            text.contains("set priority (type to filter · Enter sets"),
            "title missing: {text}"
        );
        assert!(!text.contains("[ ]"), "unexpected multi marks: {text}");
    }

    #[test]
    fn golden_labels_set_popup() {
        let mut app = picker_app(Mode::LabelsSet);
        app.multi_selected.insert(1); // "alpha"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "labels popup width");
        let text = popup.text();
        assert!(
            text.contains("set labels (type to filter · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] alpha"), "checked mark missing: {text}");
    }

    #[test]
    fn golden_pr_picker_popup() {
        let app = picker_app(Mode::PrPicker);
        let popup = popup_box(&render_app(&app, 100, 30));

        // The PR picker is the one wider popup.
        assert_eq!(popup.width, 60, "PR picker popup width");
        let text = popup.text();
        assert!(
            text.contains("linked PRs (type to filter · Enter picks · Esc cancels)"),
            "title missing: {text}"
        );
    }

    #[test]
    fn golden_issue_form_select_popup() {
        let mut app = picker_app(Mode::IssueFormSelect(4));
        app.issue_form = Some(IssueForm::new("repo".into()));
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "form select popup width");
        let text = popup.text();
        // Field name comes from ISSUE_FORM_FIELDS[4], with no "select" prefix.
        assert!(text.contains(" type ("), "title missing: {text}");
        assert!(
            text.contains("type filters · Enter picks · Esc cancels"),
            "single-select hint missing: {text}"
        );
        // Form pickers label the clear entry "none", not "clear".
        assert!(text.contains("— none —"), "none entry missing: {text}");
        assert!(!text.contains("— clear —"), "wrong clear label: {text}");
    }

    #[test]
    fn golden_issue_form_multi_popup() {
        let mut app = picker_app(Mode::IssueFormMulti(3));
        app.issue_form = Some(IssueForm::new("repo".into()));
        app.multi_selected.insert(3); // "gamma"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "form multi popup width");
        let text = popup.text();
        assert!(
            text.contains(" labels (type filters · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] gamma"), "checked mark missing: {text}");
    }

    /// Pickers are centred on the frame and sized to their content: one row
    /// per option plus the two border rows.
    #[test]
    fn golden_picker_is_centred_and_sized_to_content() {
        let popup = popup_box(&render_app(&picker_app(Mode::SelectField(1)), 100, 30));
        // 4 options + 2 borders.
        assert_eq!(popup.height, 6, "picker height");
        assert_eq!(popup.width, 50, "picker width");
        assert_eq!(popup.x, (100 - popup.width) / 2, "picker not centred in x");
        assert_eq!(popup.y, (30 - popup.height) / 2, "picker not centred in y");
    }

    /// Picker titles are longer than the popups are wide, so the border
    /// clips them. That clipping is the current appearance and any change to
    /// popup width or title wording moves it.
    #[test]
    fn golden_picker_titles_clip_at_the_border() {
        let cases = [
            (
                Mode::SelectField(1),
                "┌ select repo (type to filter · Enter picks · Esc┐",
            ),
            (
                Mode::PrioritySet,
                "┌ set priority (type to filter · Enter sets · Esc┐",
            ),
        ];
        for (mode, want) in cases {
            let popup = popup_box(&render_app(&picker_app(mode), 100, 30));
            assert_eq!(popup.rows[0], want, "clipped title for {mode:?}");
        }

        // The PR picker is wide enough that its title is not clipped.
        let popup = popup_box(&render_app(&picker_app(Mode::PrPicker), 100, 30));
        assert!(
            popup.rows[0].contains("Esc cancels) "),
            "PR picker title unexpectedly clipped: {}",
            popup.rows[0]
        );
    }

    #[test]
    fn golden_picker_type_ahead_filter_row() {
        let mut app = picker_app(Mode::SelectField(1));
        app.picker_filter_push('a');
        app.picker_filter_push('l');
        let popup = popup_box(&render_app(&app, 100, 30));

        let text = popup.text();
        assert!(text.contains("/ al"), "filter row missing: {text}");
        assert!(text.contains("alpha"), "match missing: {text}");
        assert!(!text.contains("beta"), "non-match still shown: {text}");
    }

    #[test]
    fn golden_picker_reports_empty_and_no_match_distinctly() {
        let mut empty = test_app();
        empty.start_picker(vec![], 0);
        empty.mode = Mode::SelectField(1);
        assert!(
            popup_box(&render_app(&empty, 100, 30))
                .text()
                .contains("nothing available")
        );

        let mut no_match = picker_app(Mode::SelectField(1));
        no_match.picker_filter_push('z');
        assert!(
            popup_box(&render_app(&no_match, 100, 30))
                .text()
                .contains("no matches")
        );
    }

    // ---- PR summary row model -------------------------------------------

    fn pr_summary_fixture(body: &str) -> crate::provider::types::PrSummary {
        use crate::provider::types::{
            CheckContextInfo, CheckRollup, PrRef, PrState, PrSummary, ReviewDecision,
            ReviewSummary, WorkflowRunInfo,
        };

        let run = |workflow: &str, n: u64| WorkflowRunInfo {
            workflow: workflow.into(),
            run_number: n,
            event: "push".into(),
            conclusion: Some("SUCCESS".into()),
            created_at: chrono::Utc::now(),
            url: format!("https://example.test/run/{n}"),
        };

        PrSummary {
            pr: PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 7,
            },
            title: "Add a thing".into(),
            body: body.into(),
            state: PrState::Open,
            is_draft: false,
            base_ref: "main".into(),
            head_ref: "feature".into(),
            additions: 10,
            deletions: 2,
            changed_files: 3,
            comment_count: 4,
            review_thread_count: 1,
            reviews: ReviewSummary {
                decision: Some(ReviewDecision::Approved),
                approved: 1,
                changes_requested: 0,
                commented: 2,
            },
            checks: CheckRollup {
                state: Some("SUCCESS".into()),
                contexts: vec![
                    CheckContextInfo {
                        name: "check-one".into(),
                        conclusion: "SUCCESS".into(),
                        url: "https://example.test/check/1".into(),
                    },
                    CheckContextInfo {
                        name: "check-two".into(),
                        conclusion: "FAILURE".into(),
                        url: "https://example.test/check/2".into(),
                    },
                ],
            },
            pr_runs: vec![run("pr-workflow", 11)],
            default_branch_name: "main".into(),
            default_branch_runs: vec![run("main-workflow", 22)],
        }
    }

    /// An app showing the PR summary popup for a PR with `body`.
    fn pr_summary_app(body: &str) -> App {
        use crate::provider::types::PrRef;

        let mut app = test_app();
        let pr = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 7,
        };
        app.pr_target = Some(pr);
        app.pr_summary = Some(Ok(pr_summary_fixture(body)));
        app.mode = Mode::PrSummary;
        app
    }

    /// The popup's navigable rows for an app rendered into a `frame_width`
    /// wide terminal — the same call the key handler makes.
    fn app_pr_targets(app: &App, frame_width: u16) -> Vec<PrTarget> {
        pr_targets(app.pr_summary.as_ref(), pr_summary_inner_width(frame_width))
    }

    /// Index of the popup's first content row containing `needle`, expressed
    /// in the same units as `PrTarget::line` (0 = first line under the top
    /// border, with the popup unscrolled).
    fn content_row_of(popup: &PopupBox, needle: &str) -> usize {
        popup
            .rows
            .iter()
            .skip(1) // row 0 is the top border
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not found in popup:\n{}", popup.text()))
    }

    /// With a short body — no line wide enough to wrap — every navigable
    /// target's `line` lands on the row that actually draws it.
    #[test]
    fn golden_pr_summary_targets_match_drawn_rows_short_body() {
        let app = pr_summary_app("short body line\nanother short line");
        let popup = popup_box(&render_app(&app, 100, 30));
        let targets = app_pr_targets(&app, 100);

        // PR header, 2 checks, 1 PR run, 1 default-branch run.
        assert_eq!(targets.len(), 5, "target count");

        for (i, needle) in [
            (0usize, "o/r#7"),
            (1, "check-one"),
            (2, "check-two"),
            (3, "pr-workflow"),
            (4, "main-workflow"),
        ] {
            assert_eq!(
                targets[i].line as usize,
                content_row_of(&popup, needle),
                "target {i} ({needle}) points at the wrong row:\n{}",
                popup.text()
            );
        }
    }

    /// The selected row is the one that gets the highlight background, and
    /// selection moves the scroll to that row.
    #[test]
    fn golden_pr_summary_selection_highlights_its_own_row() {
        let mut app = pr_summary_app("short body");
        let targets = app_pr_targets(&app, 100);
        app.select_pr_target(1, &targets); // PR header → first check
        assert_eq!(app.pr_sel, 1);
        assert_eq!(app.pr_scroll, targets[1].line, "scroll snaps to the target");
    }

    /// Regression for the issue #87 defect: targets used to be computed from
    /// the body's *unwrapped* source line count while the popup rendered it
    /// wrapped, so a body line wider than the popup pushed every check and
    /// run target out of step with the row that drew it. Both now come from
    /// one row model, so a wrapping body cannot shift them.
    #[test]
    fn golden_pr_summary_targets_survive_a_wrapping_body() {
        // Popup is 76 wide, so 74 columns of content: this wraps to 2 rows.
        let long = "w".repeat(100);
        let app = pr_summary_app(&long);
        let popup = popup_box(&render_app(&app, 100, 30));
        assert_eq!(popup.width, 76, "PR summary popup width");

        let targets = app_pr_targets(&app, 100);
        for (i, needle) in [
            (0usize, "o/r#7"),
            (1, "check-one"),
            (2, "check-two"),
            (3, "pr-workflow"),
            (4, "main-workflow"),
        ] {
            assert_eq!(
                targets[i].line as usize,
                content_row_of(&popup, needle),
                "target {i} ({needle}) drifted when the body wrapped:\n{}",
                popup.text()
            );
        }
    }

    /// The row model orders the navigable rows as the popup lists them: the
    /// PR header, each check, each PR run, then each default-branch run.
    #[test]
    fn golden_pr_targets_order_header_checks_runs_and_default_branch_runs() {
        let app = pr_summary_app("short body");
        let targets = app_pr_targets(&app, 100);

        assert_eq!(targets.len(), 5); // header + 2 checks + 1 pr run + 1 branch run
        assert_eq!(targets[0].url, "https://github.com/o/r/pull/7");
        assert_eq!(targets[0].line, 0);
        assert_eq!(targets[1].url, "https://example.test/check/1");
        assert_eq!(targets[2].url, "https://example.test/check/2");
        assert_eq!(targets[3].url, "https://example.test/run/11");
        assert_eq!(targets[4].url, "https://example.test/run/22");

        // Rows strictly increase — blank and heading rows sit between the
        // sections without targets of their own.
        for w in targets.windows(2) {
            assert!(w[1].line > w[0].line, "targets must be in row order");
        }
    }

    /// A wrapped logical line contributes several rows, but only its first
    /// carries the URL — selecting it scrolls to where the item starts.
    #[test]
    fn golden_wrapped_rows_tag_only_their_first_row() {
        let app = pr_summary_app("short body");
        let rows = pr_summary_rows(app.pr_summary.as_ref(), &Theme::default(), 20);
        let tagged = rows.iter().filter(|r| r.url.is_some()).count();
        assert_eq!(
            tagged, 5,
            "a narrow width wraps rows but must not multiply targets"
        );
    }

    /// A single-issue app with the detail pane open, for geometry assertions.
    fn detail_app() -> App {
        use crate::provider::types::RepoIssues;

        let mut i = issue(vec![]);
        i.body = (1..=40).map(|n| format!("body {n}\n")).collect();
        let mut app = test_app();
        app.state_filter = crate::tui::app::StateFilter::All;
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![i],
        }]);
        app.selected = 1;
        app.open_detail();
        app.detail_comments = Some(vec![test_comment("first"), test_comment("second")]);
        app
    }

    /// Column of the detail pane's left border, found by scanning the top row
    /// for the second box corner.
    fn detail_pane_x(buf: &ratatui::buffer::Buffer) -> u16 {
        (1..buf.area().width)
            .find(|&x| cell_at(buf, x, 0) == "┌")
            .expect("detail pane border not found")
    }

    /// Row of the horizontal rule separating the body region from the
    /// comments region, i.e. the body region's bottom border.
    fn detail_region_split_y(buf: &ratatui::buffer::Buffer, pane_x: u16) -> u16 {
        (1..buf.area().height)
            .find(|&y| cell_at(buf, pane_x, y) == "└")
            .expect("body region bottom border not found")
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
