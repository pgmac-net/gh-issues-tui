use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::github::types::Issue;

use super::app::{
    App, BODY_POPUP_WIDTH, FILTER_FIELDS, Focus, ISSUE_FORM_CREATE_ROW, ISSUE_FORM_FIELDS,
    InputKind, Mode, Row, body_popup_width, cursor_row, wrap_lines,
};
use super::theme::Theme;

pub fn draw(f: &mut Frame, app: &App, t: &Theme) {
    let [main, info, bottom] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    if app.detail_open {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        draw_list(f, app, t, left);
        draw_detail(f, app, t, right);
    } else {
        draw_list(f, app, t, main);
    }
    draw_info_bar(f, app, t, info);
    draw_bottom_line(f, app, t, bottom);

    match app.mode {
        Mode::FilterMenu => draw_filter_menu(f, app, t),
        Mode::SelectField(idx) => draw_select_popup(f, app, t, idx),
        Mode::Calendar(idx) => draw_calendar_popup(f, app, t, idx),
        Mode::IssueForm => draw_issue_form(f, app, t),
        Mode::IssueFormSelect(idx) => {
            draw_issue_form(f, app, t);
            draw_form_choice_popup(f, app, t, idx, false);
        }
        Mode::IssueFormMulti(idx) => {
            draw_issue_form(f, app, t);
            draw_form_choice_popup(f, app, t, idx, true);
        }
        Mode::IssueFormBody => {
            draw_issue_form(f, app, t);
            draw_form_body_popup(f, app, t);
        }
        Mode::PrioritySet => draw_priority_popup(f, app, t),
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
        crate::github::types::IssueState::Open => Span::styled("●", Style::default().fg(t.open)),
        crate::github::types::IssueState::Closed => {
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(" issue (Tab switch · j/k scroll · Esc close) ");
    let Some(issue) = app.selected_issue() else {
        // Live follow landed on a repo header (or an empty list).
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
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("#{} ", issue.number), Style::default().fg(t.dim)),
            Span::styled(
                issue.title.clone(),
                title_style(issue, t).add_modifier(Modifier::BOLD),
            ),
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

    for l in issue.body.lines() {
        lines.push(Line::raw(l.to_string()));
    }

    lines.push(Line::default());
    match &app.detail_comments {
        None => lines.push(Line::styled(
            "loading comments…",
            Style::default().fg(t.dim),
        )),
        Some(comments) if comments.is_empty() => {
            lines.push(Line::styled("no comments", Style::default().fg(t.dim)));
        }
        Some(comments) => {
            for c in comments {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("── {} ", c.author),
                        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", c.created_at.format("%Y-%m-%d %H:%M")),
                        Style::default().fg(t.dim),
                    ),
                ]));
                for l in c.body.lines() {
                    lines.push(Line::raw(l.to_string()));
                }
                lines.push(Line::default());
            }
        }
    }

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(para, area);
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
        crate::github::types::IssueState::Open => Style::default().fg(t.open),
        crate::github::types::IssueState::Closed => Style::default().fg(t.closed),
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

fn draw_bottom_line(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    match app.mode {
        Mode::Input(kind) => {
            let prompt = match kind {
                InputKind::Search => "search",
                InputKind::FilterField(idx) => FILTER_FIELDS[idx],
                InputKind::Comment => "comment (Enter submits)",
                InputKind::Assignees => "assignees (comma-separated logins)",
                InputKind::Labels => "labels (comma-separated)",
                InputKind::Title => "title",
                InputKind::Org => "org/owner (Enter switches)",
                InputKind::FormTitle => "issue title (Enter sets)",
            };
            let mut spans = vec![Span::styled(
                format!(" {prompt}: "),
                Style::default().fg(t.accent),
            )];
            spans.extend(cursor_spans(&app.input.buffer, app.input.cursor));
            f.render_widget(Paragraph::new(Line::from(spans)), area);
        }
        Mode::ConfirmState => {
            let action = app
                .selected_issue()
                .map(|i| match i.state {
                    crate::github::types::IssueState::Open => "close",
                    crate::github::types::IssueState::Closed => "reopen",
                })
                .unwrap_or("toggle");
            f.render_widget(
                Paragraph::new(Line::styled(
                    format!(" {action} this issue? y/n"),
                    Style::default().fg(t.warning),
                )),
                area,
            );
        }
        _ => {
            let msg = app.status.clone().unwrap_or_default();
            f.render_widget(
                Paragraph::new(Line::styled(format!(" {msg}"), Style::default().fg(t.dim))),
                area,
            );
        }
    }
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

fn draw_select_popup(f: &mut Frame, app: &App, t: &Theme, idx: usize) {
    let field_name = FILTER_FIELDS[idx];
    let items = picker_items(app, t, false, "clear");
    let area = centered(f.area(), 50, picker_height(f, items.len()));
    f.render_widget(Clear, area);
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
        " select {field_name} (type to filter · Enter picks · Esc cancels) "
    )));
    f.render_widget(list, area);
}

fn draw_priority_popup(f: &mut Frame, app: &App, t: &Theme) {
    let items = picker_items(app, t, false, "clear");
    let area = centered(f.area(), 50, picker_height(f, items.len()));
    f.render_widget(Clear, area);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" set priority (type to filter · Enter sets · Esc cancels) "),
    );
    f.render_widget(list, area);
}

fn draw_issue_form(f: &mut Frame, app: &App, t: &Theme) {
    let Some(form) = &app.issue_form else { return };
    let area = centered(f.area(), 70, ISSUE_FORM_FIELDS.len() as u16 + 4);
    f.render_widget(Clear, area);

    let mut items: Vec<ListItem> = ISSUE_FORM_FIELDS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let style = if i == form.field_idx {
                Style::default().bg(t.selected_bg)
            } else {
                Style::default()
            };
            let value = if form.options.is_none() && i >= 2 {
                "loading…".to_string()
            } else {
                form.field_display(i)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<14}"), style.fg(t.accent)),
                Span::styled(value, style),
            ]))
        })
        .collect();

    let create_style = if form.field_idx == ISSUE_FORM_CREATE_ROW {
        Style::default()
            .bg(t.selected_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    items.push(ListItem::new(Line::raw("")));
    items.push(ListItem::new(Line::from(Span::styled(
        " [ Create issue ]",
        create_style.fg(t.open),
    ))));

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(
                " new issue in {} (Enter edit · Esc cancel) ",
                form.repo
            )),
    );
    f.render_widget(list, area);
}

/// Option popup for a form field: single-select (with the "—" clear row)
/// or multi-select (Space toggles, checkbox markers).
fn draw_form_choice_popup(f: &mut Frame, app: &App, t: &Theme, idx: usize, multi: bool) {
    let field_name = ISSUE_FORM_FIELDS[idx];
    let items = picker_items(app, t, multi, "none");
    let area = centered(f.area(), 50, picker_height(f, items.len()));
    f.render_widget(Clear, area);
    let hint = if multi {
        "type filters · Space toggles · Enter accepts"
    } else {
        "type filters · Enter picks · Esc cancels"
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {field_name} ({hint}) ")),
    );
    f.render_widget(list, area);
}

fn draw_form_body_popup(f: &mut Frame, app: &App, t: &Theme) {
    let Some(form) = &app.issue_form else { return };
    let area = centered(f.area(), BODY_POPUP_WIDTH, 18.min(f.area().height));
    f.render_widget(Clear, area);
    let inner_height = area.height.saturating_sub(2) as usize;
    let width = body_popup_width(f.area().width) as usize;

    // Word-wrapped visual rows; keep the cursor's row visible.
    let rows = wrap_lines(&form.body.lines, width);
    let (cur_row, cur_col) = cursor_row(
        &rows,
        form.body.line,
        form.body.lines[form.body.line].cursor,
    );
    let top = cur_row.saturating_sub(inner_height.saturating_sub(1));
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(inner_height)
        .map(|(i, row)| {
            let text: String = form.body.lines[row.line]
                .buffer
                .chars()
                .skip(row.start)
                .take(row.end - row.start)
                .collect();
            if i == cur_row {
                Line::from(cursor_spans(&text, cur_col))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(" description (Enter newline · Esc done) "),
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
        ("j/k ↑/↓", "move"),
        ("Space", "collapse/expand repo group"),
        ("←", "collapse repo group / back to list"),
        ("→", "expand repo group / into detail pane"),
        ("[ / ]", "collapse all / expand all"),
        ("Enter", "open issue in detail pane"),
        ("Tab", "switch pane (Shift+Tab reverse)"),
        ("Esc / q", "close detail pane"),
        ("o / O", "open issue / repo in browser"),
        ("/", "text search"),
        ("f", "cycle state filter (open/closed/all)"),
        ("F", "filter editor (pickers + calendar)"),
        ("s / S", "cycle sort key / toggle direction"),
        ("w", "switch org/owner"),
        ("c", "add comment"),
        ("x", "close / reopen issue"),
        ("a", "edit assignees"),
        ("l", "edit labels"),
        ("t", "edit title"),
        ("p", "set priority"),
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
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" keys ")),
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
    use crate::github::types::{Issue, IssueState, Label};

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
}
