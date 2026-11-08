use super::prelude::*;
use super::widgets::label_color;
use crate::tui::app::FILTER_FIELDS;
use crate::tui::app::search::SemanticIndicator;
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
                depth,
                branch,
            } => {
                let issue = &app.repos[*repo_idx].issues[*issue_idx];
                issue_item(issue, &tree_lead(app, &issue.id, *depth, *branch), t)
            }
            Row::Context {
                repo_idx,
                depth,
                branch,
                ..
            } => context_item(app, row, *repo_idx, *depth, *branch, t),
        })
        .collect();

    // A snapshot on screen is the list, not a placeholder: count it (#226).
    let title = if app.loading && app.cached.is_none() {
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
        .highlight_style(Style::default().bg(selection_bg(app, t, Focus::List)))
        .highlight_symbol("> ");

    let height = layout::inner_height(area) as usize;
    let top = layout::scroll_top(app.list_top.get(), app.selected, height, app.rows.len());
    app.list_top.set(top);
    let mut state = ListState::default().with_offset(top);
    if !app.rows.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// What goes before a list row's state dot (#234): a space, two per level of
/// sub-issue nesting, then `▾`/`▸` on a parent or two spaces on a leaf. A
/// top-level leaf gets the three spaces every issue row always had.
fn tree_lead(app: &App, id: &str, depth: u8, branch: bool) -> String {
    let marker = match (branch, app.collapsed_issues.contains(id)) {
        (false, _) => "  ",
        (true, false) => "▾ ",
        (true, true) => "▸ ",
    };
    format!(" {}{marker}", "  ".repeat(depth as usize))
}

/// Sub-issue progress after a parent's title (#234): GitHub's own count, so it
/// includes children that are closed, filtered out or in another repo.
fn progress_span(issue: &Issue, style: Style) -> Option<Span<'static>> {
    issue
        .sub_issues
        .map(|p| Span::styled(format!("  {}/{}", p.completed, p.total), style))
}

pub(super) fn issue_item(issue: &Issue, lead: &str, t: &Theme) -> ListItem<'static> {
    let state_span = match issue.state {
        crate::provider::types::IssueState::Open => Span::styled("●", Style::default().fg(t.open)),
        crate::provider::types::IssueState::Closed => {
            Span::styled("●", Style::default().fg(t.closed))
        }
    };
    let mut spans = vec![
        Span::raw(lead.to_string()),
        state_span,
        Span::styled(format!(" #{:<5}", issue.number), Style::default().fg(t.dim)),
        Span::styled(issue.title.clone(), title_style(issue, t)),
    ];
    spans.extend(progress_span(issue, Style::default().fg(t.dim)));
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
        format!("  {}", crate::localtime::fmt_date(issue.updated_at)),
        Style::default().fg(t.dim),
    ));
    ListItem::new(Line::from(spans))
}

/// A context row (#234): an ancestor drawn only so its descendants keep their
/// place. All dim, with no labels, assignees or dates, so it reads as
/// background. A repo prefix names where it lives when that isn't this group.
fn context_item(
    app: &App,
    row: &Row,
    group_idx: usize,
    depth: u8,
    branch: bool,
    t: &Theme,
) -> ListItem<'static> {
    let dim = Style::default().fg(t.dim);
    let group = &app.repos[group_idx].repo;
    let (id, number, title, home, progress) = if let Some(issue) = app.context_issue(row) {
        // An outside sub-issue (#261) names its own repo, and owner when
        // that is not the browsed org; a loaded one its group.
        let home = match app.row_outside(row) {
            Some(o) if o.owner.is_empty() || o.owner.eq_ignore_ascii_case(&app.org) => {
                o.repo.clone()
            }
            Some(o) => format!("{}/{}", o.owner, o.repo),
            None => app.issue_repo(&issue.id).unwrap_or(group).to_string(),
        };
        (
            issue.id.clone(),
            issue.number,
            issue.title.clone(),
            home,
            progress_span(issue, dim),
        )
    } else if let Some(stub) = app.row_stub(row) {
        let home = if stub.owner.is_empty() || stub.owner.eq_ignore_ascii_case(&app.org) {
            stub.repo.clone()
        } else {
            format!("{}/{}", stub.owner, stub.repo)
        };
        (stub.id.clone(), stub.number, stub.title.clone(), home, None)
    } else {
        return ListItem::new(Line::default());
    };
    let reference = if home == *group {
        format!(" #{number:<5}")
    } else {
        format!(" {home}#{number} ")
    };
    let mut spans = vec![
        Span::raw(tree_lead(app, &id, depth, branch)),
        Span::styled("○", dim),
        Span::styled(reference, dim),
        Span::styled(title, dim.add_modifier(Modifier::ITALIC)),
    ];
    spans.extend(progress);
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

/// Selection background for a pane: dimmed while the split is open and
/// another pane has focus (#217), so only the focused pane's selection is
/// loud.
pub(super) fn selection_bg(app: &App, t: &Theme, pane: Focus) -> Color {
    if app.detail.open && app.focus != pane {
        t.selected_bg_unfocused
    } else {
        t.selected_bg
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
    // Column 0 is the refresh indicator (#250): blank when idle, so the bar
    // never shifts.
    let indicator = match app.loading_elapsed(std::time::Instant::now()) {
        Some(elapsed) => Span::styled(
            app.spinner.glyph(elapsed).to_string(),
            Style::default().fg(app.spinner.colour(elapsed)),
        ),
        None => Span::raw(" "),
    };
    let mut spans = vec![
        indicator,
        Span::styled("state:", Style::default().fg(t.dim)),
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
    // Showing a snapshot no fetch has replaced yet (#226): how old it is.
    if let Some(c) = app.cache_indicator(chrono::Utc::now()) {
        let color = if app.loading { t.dim } else { t.warning };
        spans.push(Span::styled(format!("  {c}"), Style::default().fg(color)));
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
    // Semantic search (#184): nothing on screen said it was on, or working, or
    // had failed. Only while a query that would be sent is typed.
    if let Some(s) = app.semantic_indicator() {
        let (text, colour) = match s {
            SemanticIndicator::Searching => ("semantic: searching…".to_string(), t.dim),
            SemanticIndicator::Added(n) => (format!("semantic: +{n}"), t.accent),
            SemanticIndicator::Off(why) if why.is_empty() => {
                ("semantic: off".to_string(), t.warning)
            }
            SemanticIndicator::Off(why) => (format!("semantic: off ({why})"), t.warning),
        };
        spans.push(Span::styled(
            format!("  {text}"),
            Style::default().fg(colour),
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
        InputKind::RunSearch => "search runs (workflow, title, branch, actor, #)",
    }
}

pub(super) fn draw_bottom_line(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    // A sticky startup warning outranks the status line (#250).
    let mut spans = if let Some(w) = &app.config_warning {
        vec![Span::styled(
            format!(" ⚠ {w}"),
            Style::default().fg(t.warning),
        )]
    } else {
        // With nothing to report, the repo workflows view puts its selected
        // run's title and actor here (#266): the row has no room for them.
        let msg = app
            .status
            .clone()
            .or_else(|| super::repo_runs::caption(app))
            .unwrap_or_default();
        vec![Span::styled(format!(" {msg}"), Style::default().fg(t.dim))]
    };
    // Detached harness sessions (#23) are otherwise invisible — without this
    // an agent could be running with nothing on screen to say so.
    if let Some(segment) = app.harness.status_segment() {
        spans.push(Span::styled(
            format!("  ·  {segment} (Z)"),
            Style::default().fg(t.accent),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
                ..Default::default()
            },
            Label {
                name: "priority:high".into(),
                color: "d93f0b".into(),
                ..Default::default()
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
            ..Default::default()
        }]);
        assert_eq!(title_style(&i, &Theme::default()).fg, None);
    }

    #[test]
    fn moving_up_inside_the_viewport_does_not_scroll_it() {
        use crate::provider::types::RepoIssues;
        let issues: Vec<Issue> = (1..=40)
            .map(|n| {
                let mut i = issue(vec![]);
                i.number = n;
                i
            })
            .collect();
        let mut app = test_app();
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues,
        }]);
        // Scroll down past the first page, one frame per step.
        for sel in 1..=30 {
            app.selected = sel;
            render_app(&app, 80, 14);
        }
        let top = app.list_top.get();
        assert!(top > 0, "list should have scrolled");
        // Moving up within the window must leave the viewport where it is.
        app.selected -= 1;
        render_app(&app, 80, 14);
        assert_eq!(app.list_top.get(), top);
        // Moving above the window scrolls it just far enough.
        app.selected = top - 1;
        render_app(&app, 80, 14);
        assert_eq!(app.list_top.get(), top - 1);
    }
}

#[cfg(test)]
mod indicator_tests {
    use super::super::testutil::*;
    use crate::tui::spinner::DEFAULT_GLYPHS;

    fn row(buf: &ratatui::buffer::Buffer, y: u16, w: u16) -> String {
        (0..w).map(|x| cell_at(buf, x, y)).collect()
    }

    fn busy_app() -> crate::tui::app::App {
        let mut app = test_app();
        app.loading = true;
        app.loading_since = Some(std::time::Instant::now());
        app
    }

    #[test]
    fn column_zero_of_the_info_bar_spins_while_loading() {
        let buf = render_app(&busy_app(), 80, 20);
        let glyph = cell_at(&buf, 0, 18);
        assert!(DEFAULT_GLYPHS.contains(&glyph), "{glyph:?}");
        assert!(row(&buf, 18, 80).contains("state:"));
    }

    #[test]
    fn column_zero_is_blank_when_idle() {
        let mut app = test_app();
        app.loading = false;
        let buf = render_app(&app, 80, 20);
        assert_eq!(cell_at(&buf, 0, 18), " ");
        assert!(row(&buf, 18, 80).starts_with(" state:"));
    }

    #[test]
    fn the_indicator_does_not_shift_the_bar() {
        let mut idle = test_app();
        idle.loading = false;
        let a = row(&render_app(&idle, 80, 20), 18, 80);
        let b = row(&render_app(&busy_app(), 80, 20), 18, 80);
        assert_eq!(
            a.chars().skip(1).collect::<String>(),
            b.chars().skip(1).collect::<String>()
        );
    }

    #[test]
    fn the_glyph_is_coloured_from_the_hue() {
        let buf = render_app(&busy_app(), 80, 20);
        assert!(matches!(buf[(0, 18)].fg, ratatui::style::Color::Rgb(..)));
    }

    #[test]
    fn a_config_warning_replaces_the_status_line_until_cleared() {
        let mut app = test_app();
        app.status = Some("loaded 3 issues across 1 repos".into());
        app.config_warning = Some("spinner_glyphs: is empty — using default".into());
        let bottom = row(&render_app(&app, 100, 20), 19, 100);
        assert!(bottom.contains("⚠ spinner_glyphs: is empty"), "{bottom}");
        assert!(!bottom.contains("loaded 3"));

        app.config_warning = None;
        let bottom = row(&render_app(&app, 100, 20), 19, 100);
        assert!(bottom.contains("loaded 3 issues"), "{bottom}");
    }
}
