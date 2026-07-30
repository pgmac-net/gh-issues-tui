use super::prelude::*;
use super::widgets::label_color;
use crate::tui::app::FILTER_FIELDS;
use crate::tui::app::{Focus, InputKind, Row};
use ratatui::widgets::ListState;

/// Title colour: the priority label's own colour when one is set, default otherwise.
pub(super) fn draw_list(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
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

pub(super) fn issue_item(issue: &Issue, t: &Theme) -> ListItem<'static> {
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
pub(super) fn pane_border(app: &App, t: &Theme, pane: Focus) -> Style {
    if app.detail.open && app.focus == pane {
        Style::default().fg(t.accent)
    } else {
        Style::default()
    }
}

pub(super) fn title_style(issue: &Issue, t: &Theme) -> Style {
    match issue.priority_label() {
        Some(l) => Style::default().fg(label_color(&l.color, t.label_fallback)),
        None => Style::default(),
    }
}

pub(super) fn state_style(issue: &Issue, t: &Theme) -> Style {
    match issue.state {
        crate::provider::types::IssueState::Open => Style::default().fg(t.open),
        crate::provider::types::IssueState::Closed => Style::default().fg(t.closed),
    }
}

pub(super) fn draw_info_bar(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
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
        // The per-fetch cost is what actually burns the budget, so show it
        // next to the remaining balance when the backend reports one (#107).
        let cost = match rl.last_cost {
            Some(c) => format!(" (last fetch {c})"),
            None => String::new(),
        };
        spans.push(Span::styled(
            format!("  API {}/{}{}", rl.remaining, rl.limit, cost),
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
pub(super) fn input_prompt(kind: InputKind) -> &'static str {
    match kind {
        InputKind::Search => "search",
        InputKind::FilterField(idx) => FILTER_FIELDS[idx],
        InputKind::Assignees => "assignees (comma-separated logins)",
        InputKind::Title => "title",
        InputKind::Org => "org/owner (Enter switches)",
        InputKind::GotoNumber => "issue # (Enter jumps)",
    }
}

pub(super) fn draw_bottom_line(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let msg = app.status.clone().unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {msg}"), Style::default().fg(t.dim))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::provider::types::Label;

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
