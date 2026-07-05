use chrono::{Datelike, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::github::types::Issue;

use super::app::{App, FILTER_FIELDS, Focus, InputKind, Mode, Row};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &App) {
    let [main, info, bottom] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    match app.focus {
        Focus::List => draw_list(f, app, main),
        Focus::Detail => draw_detail(f, app, main),
    }
    draw_info_bar(f, app, info);
    draw_bottom_line(f, app, bottom);

    match app.mode {
        Mode::FilterMenu => draw_filter_menu(f, app),
        Mode::SelectField(idx) => draw_select_popup(f, app, idx),
        Mode::Calendar(idx) => draw_calendar_popup(f, app, idx),
        Mode::Help => draw_help(f),
        _ => {}
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
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
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ({})", app.repo_visible_count(*repo_idx)),
                        Style::default().fg(DIM),
                    ),
                ]))
            }
            Row::Issue {
                repo_idx,
                issue_idx,
            } => issue_item(&app.repos[*repo_idx].issues[*issue_idx]),
        })
        .collect();

    let title = if app.loading {
        format!(" {} — loading… ", app.org)
    } else {
        format!(" {} — {} issues ", app.org, app.visible_issue_count())
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    if !app.rows.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn issue_item(issue: &Issue) -> ListItem<'static> {
    let state_span = match issue.state {
        crate::github::types::IssueState::Open => {
            Span::styled("●", Style::default().fg(Color::Green))
        }
        crate::github::types::IssueState::Closed => {
            Span::styled("●", Style::default().fg(Color::Magenta))
        }
    };
    let mut spans = vec![
        Span::raw("   "),
        state_span,
        Span::styled(format!(" #{:<5}", issue.number), Style::default().fg(DIM)),
        Span::raw(issue.title.clone()),
    ];
    if !issue.assignees.is_empty() {
        spans.push(Span::styled(
            format!("  @{}", issue.assignees.join(",@")),
            Style::default().fg(Color::Yellow),
        ));
    }
    for label in &issue.labels {
        spans.push(Span::styled(
            format!(" [{}]", label.name),
            Style::default().fg(label_color(&label.color)),
        ));
    }
    if issue.comment_count > 0 {
        spans.push(Span::styled(
            format!(" 🗨{}", issue.comment_count),
            Style::default().fg(DIM),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", issue.updated_at.format("%Y-%m-%d")),
        Style::default().fg(DIM),
    ));
    ListItem::new(Line::from(spans))
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(issue) = app.selected_issue() else {
        return;
    };
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("#{} ", issue.number), Style::default().fg(DIM)),
            Span::styled(
                issue.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", issue.state), state_style(issue)),
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
                Style::default().fg(DIM),
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
            Style::default().fg(Color::Yellow),
        )),
        Line::default(),
    ];

    for l in issue.body.lines() {
        lines.push(Line::raw(l.to_string()));
    }

    lines.push(Line::default());
    match &app.detail_comments {
        None => lines.push(Line::styled("loading comments…", Style::default().fg(DIM))),
        Some(comments) if comments.is_empty() => {
            lines.push(Line::styled("no comments", Style::default().fg(DIM)));
        }
        Some(comments) => {
            for c in comments {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("── {} ", c.author),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", c.created_at.format("%Y-%m-%d %H:%M")),
                        Style::default().fg(DIM),
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" issue detail (Esc back, j/k scroll) "),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(para, area);
}

fn state_style(issue: &Issue) -> Style {
    match issue.state {
        crate::github::types::IssueState::Open => Style::default().fg(Color::Green),
        crate::github::types::IssueState::Closed => Style::default().fg(Color::Magenta),
    }
}

fn draw_info_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" state:", Style::default().fg(DIM)),
        Span::raw(app.state_filter.label()),
        Span::styled("  sort:", Style::default().fg(DIM)),
        Span::raw(format!(
            "{}{}",
            app.sort_key.label(),
            if app.sort_desc { "↓" } else { "↑" }
        )),
    ];
    // Rate limit indicator
    if let Some(rl) = &app.rate_limit {
        let color = if rl.remaining < 10 {
            Color::Red
        } else if rl.remaining < 100 {
            Color::Yellow
        } else {
            DIM
        };
        spans.push(Span::styled(
            format!("  API {}/{}", rl.remaining, rl.limit),
            Style::default().fg(color),
        ));
    }
    if let Some(err) = &app.rate_limit_error {
        spans.push(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(Color::Red),
        ));
    } else if app.filters.is_active() {
        spans.push(Span::styled(
            "  [filters active — F to edit, F→c to clear]",
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled("  ?:help", Style::default().fg(DIM)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_bottom_line(f: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Input(kind) => {
            let prompt = match kind {
                InputKind::Search => "search",
                InputKind::FilterField(idx) => FILTER_FIELDS[idx],
                InputKind::Comment => "comment (Enter submits)",
                InputKind::Assignees => "assignees (comma-separated logins)",
                InputKind::Labels => "labels (comma-separated)",
                InputKind::Title => "title",
            };
            let line = Line::from(vec![
                Span::styled(format!(" {prompt}: "), Style::default().fg(ACCENT)),
                Span::raw(app.input.buffer.clone()),
                Span::styled("█", Style::default().fg(ACCENT)),
            ]);
            f.render_widget(Paragraph::new(line), area);
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
                    Style::default().fg(Color::Yellow),
                )),
                area,
            );
        }
        _ => {
            let msg = app.status.clone().unwrap_or_default();
            f.render_widget(
                Paragraph::new(Line::styled(format!(" {msg}"), Style::default().fg(DIM))),
                area,
            );
        }
    }
}

fn draw_filter_menu(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 60, FILTER_FIELDS.len() as u16 + 4);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = FILTER_FIELDS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let value = app.current_filter_value(i);
            let style = if i == app.filter_menu_idx {
                Style::default().bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<28}"), style.fg(ACCENT)),
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

fn draw_select_popup(f: &mut Frame, app: &App, idx: usize) {
    let field_name = FILTER_FIELDS[idx];
    let h = app.select_options.len() as u16 + 2;
    let area = centered(f.area(), 50, h);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .select_options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let style = if i == app.select_idx {
                Style::default().bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default()
            };
            let prefix = if opt == "\u{2014}" {
                "\u{2014} clear \u{2014}"
            } else {
                opt
            };
            ListItem::new(Line::from(vec![Span::styled(format!(" {prefix}"), style)]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" select {field_name} (Enter picks, Esc cancels) ")),
    );
    f.render_widget(list, area);
}

fn draw_calendar_popup(f: &mut Frame, app: &App, idx: usize) {
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
                        .bg(Color::Rgb(40, 40, 60))
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
        Span::styled("\u{2190}\u{2192} day  ", Style::default().fg(DIM)),
        Span::styled("\u{2191}\u{2193} week  ", Style::default().fg(DIM)),
        Span::styled("PgUp/PgDn month  ", Style::default().fg(DIM)),
        Span::styled("Enter select  Esc cancel", Style::default().fg(DIM)),
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

fn draw_help(f: &mut Frame) {
    const HELP: &[(&str, &str)] = &[
        ("j/k ↑/↓", "move"),
        ("Space", "collapse/expand repo group"),
        ("[ / ]", "collapse all / expand all"),
        ("Enter", "issue detail (comments)"),
        ("o / O", "open issue / repo in browser"),
        ("/", "text search"),
        ("f", "cycle state filter (open/closed/all)"),
        ("F", "filter editor"),
        ("s / S", "cycle sort key / toggle direction"),
        ("c", "add comment"),
        ("x", "close / reopen issue"),
        ("a", "edit assignees"),
        ("l", "edit labels"),
        ("t", "edit title"),
        ("r", "reload"),
        ("q", "back / quit"),
    ];
    let area = centered(f.area(), 52, HELP.len() as u16 + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), Style::default().fg(ACCENT).bold()),
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
fn label_color(hex: &str) -> Color {
    if hex.len() == 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        )
    {
        return Color::Rgb(r, g, b);
    }
    Color::Blue
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
