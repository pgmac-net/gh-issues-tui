//! The repo workflows view (#266): every recent run of the selected repo, in
//! the whole detail pane, grouped one level deep.
//!
//! Rows are the issue tab's: one screen row each, the name truncated and the
//! figures on the right whole, and jobs and steps drawn by the same code
//! (`runs.rs`), so the two views read alike.

use chrono::TimeDelta;

use super::detail::Highlight;
use super::list::pane_border;
use super::prelude::*;
use super::runs::{
    Fig, RowText, eta_figures, finished, job_text, seg, status_mark, step_text, tree_line,
};
use super::widgets::render_region_scrollbar;
use crate::ci::{CiRun, RunFilter, fmt_dur};
use crate::tui::app::{Focus, GroupKey, Grouping, RepoRowKind, RepoRunsData, RepoView};

pub(super) fn draw_repo_runs(f: &mut Frame, app: &App, t: &Theme, area: Rect, repo: &str) {
    let s = &app.repo_runs;
    let focused = app.focus == Focus::Detail;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(Line::from(vec![
            Span::styled(
                format!(" workflows · {repo} "),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· group: {} (v) ", s.group.label()),
                Style::default().fg(t.dim),
            ),
        ]));
    let (note, warn) = footnote(app);
    if !note.is_empty() {
        let colour = if warn { t.warning } else { t.dim };
        block = block.title_bottom(Line::styled(note, Style::default().fg(colour)));
    }
    let message = |text: String, colour: Color| Line::styled(text, Style::default().fg(colour));
    let data = match &s.data {
        None => {
            let text = message("loading runs…".into(), t.dim);
            f.render_widget(Paragraph::new(text).block(block), area);
            return;
        }
        Some(Err(e)) => {
            let text = message(format!("runs failed: {e}"), t.error);
            f.render_widget(Paragraph::new(text).block(block), area);
            return;
        }
        Some(Ok(data)) => data,
    };

    let view = s.view();
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let sel = s.sel.min(view.rows.len().saturating_sub(1));
    let top = layout::scroll_top(s.top.get(), sel, inner_h, view.rows.len());
    s.top.set(top);

    let lines: Vec<Line> = view
        .rows
        .iter()
        .enumerate()
        .skip(top)
        .take(inner_h)
        .map(|(i, row)| {
            let text = row_text(data, &view, row.kind, s.group, t);
            let hl = Highlight::of(i == sel, focused);
            tree_line(
                text,
                row.depth,
                (row.expandable, row.expanded),
                hl,
                inner_w,
                t,
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
    let total = u16::try_from(view.rows.len()).unwrap_or(u16::MAX);
    let scroll = u16::try_from(top).unwrap_or(u16::MAX);
    render_region_scrollbar(f, t, area, total, inner_h as u16, scroll);
}

/// The bottom border: how much is loaded, what the view is narrowed to, and
/// why it may be behind. The second value says it is a problem.
fn footnote(app: &App) -> (String, bool) {
    let s = &app.repo_runs;
    if s.paused {
        return (" polling paused: REST budget low ".into(), true);
    }
    if let Some(e) = &s.stale {
        return (format!(" refresh failed: {e} "), true);
    }
    let mut parts = Vec::new();
    if let Some(Ok(d)) = &s.data {
        parts.push(format!("{} of {}", d.runs.len(), d.total));
    }
    let filter = s.filter();
    parts.extend(filter_words(&filter.run));
    if !filter.text.is_empty() {
        parts.push(format!("/{}", filter.text));
    }
    if parts.is_empty() {
        return (String::new(), false);
    }
    (format!(" {} ", parts.join(" · ")), false)
}

/// A run filter as the words the border shows: `status:failure`, …
pub(super) fn filter_words(f: &RunFilter) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = f.status {
        out.push(format!("status:{}", s.word()));
    }
    if let Some(w) = &f.workflow {
        out.push(format!("workflow:{}", w.name));
    }
    for (name, value) in [
        ("event", &f.event),
        ("branch", &f.branch),
        ("actor", &f.actor),
    ] {
        if let Some(v) = value {
            out.push(format!("{name}:{v}"));
        }
    }
    out
}

fn row_text(
    data: &RepoRunsData,
    view: &RepoView,
    kind: RepoRowKind,
    group: Grouping,
    t: &Theme,
) -> RowText {
    let notice = |name: String, colour| RowText {
        glyph: " ",
        colour,
        name,
        right: Vec::new(),
    };
    match kind {
        RepoRowKind::Group { group: g } => {
            group_text(data, &view.groups[g].key, &view.groups[g].runs, t)
        }
        RepoRowKind::Run { run } => run_text(&data.runs[run], data, group, t),
        RepoRowKind::Job { run, job } => {
            let r = &data.runs[run];
            let estimate = data.estimates.get(&r.id).and_then(|e| e.jobs.get(job));
            job_text(&r.jobs[job], estimate, data.as_of, t)
        }
        RepoRowKind::Step { run, job, step } => {
            step_text(&data.runs[run].jobs[job].steps[step], data.as_of, t)
        }
        RepoRowKind::LoadingJobs { .. } => notice("loading jobs…".into(), t.dim),
        RepoRowKind::JobsFailed { .. } => notice("jobs failed (r retries)".into(), t.error),
        RepoRowKind::More(n) => notice(format!("+{n} more (L)"), t.dim),
        RepoRowKind::Empty => notice("no runs match".into(), t.dim),
    }
}

fn group_text(data: &RepoRunsData, key: &GroupKey, runs: &[usize], t: &Theme) -> RowText {
    let any = |pred: fn(&CiRun) -> bool| runs.iter().any(|&r| pred(&data.runs[r]));
    let (glyph, colour) = match key {
        GroupKey::Status(b) => status_mark(b.status(), t),
        _ if any(CiRun::is_active) => ("●", t.accent),
        _ if any(CiRun::is_failed) => ("✗", t.error),
        _ => ("✓", t.open),
    };
    let n = runs.len();
    RowText {
        glyph,
        colour,
        name: key.label(),
        right: vec![seg(
            Fig::Dim,
            if n == 1 {
                "1 run".into()
            } else {
                format!("{n} runs")
            },
        )],
    }
}

/// `CI #412 · main · push`, without whatever the view is grouped by.
fn run_name(run: &CiRun, group: Grouping) -> String {
    let mut parts = Vec::new();
    let head = format!("#{}", run.run_number);
    if group == Grouping::Workflow {
        parts.push(head);
    } else {
        parts.push(format!("{} {head}", run.workflow()));
    }
    if group != Grouping::Branch
        && let Some(b) = &run.branch
    {
        parts.push(b.clone());
    }
    parts.push(run.event.clone());
    parts.join(" · ")
}

fn run_text(run: &CiRun, data: &RepoRunsData, group: Grouping, t: &Theme) -> RowText {
    let (glyph, colour) = status_mark(run.status, t);
    let right = match data.estimates.get(&run.id) {
        Some(e) => {
            let mut right = Vec::new();
            if !run.jobs.is_empty() {
                let done = run.jobs.iter().filter(|j| j.status.is_done()).count();
                right.push(seg(Fig::Progress, format!("{done}/{}", run.jobs.len())));
            }
            right.push(seg(Fig::Elapsed, fmt_dur(e.elapsed)));
            right.extend(eta_figures(e));
            right
        }
        None => {
            let took = run
                .run_started_at
                .zip(run.updated_at)
                .map(|(start, end)| fmt_dur(end - start))
                .unwrap_or_default();
            let mut right = finished(run.status, &took);
            if let Some(age) = run.created_at.or(run.run_started_at) {
                right.push(seg(Fig::Dim, ago(data.as_of - age)));
            }
            right
        }
    };
    RowText {
        glyph,
        colour,
        name: run_name(run, group),
        right,
    }
}

/// How long ago, the way a list of runs reads: `5m ago`, `3h ago`, `2d ago`.
fn ago(d: TimeDelta) -> String {
    let mins = d.num_minutes().max(0);
    match mins {
        0 => "just now".into(),
        1..60 => format!("{mins}m ago"),
        60..1440 => format!("{}h ago", mins / 60),
        _ => format!("{}d ago", mins / 1440),
    }
}

/// The selected run's title and actor, for the bottom line: the row has no
/// room for them.
pub(super) fn caption(app: &App) -> Option<String> {
    if !app.repo_view_showing() || app.focus != Focus::Detail {
        return None;
    }
    let run = app.repo_runs.selected_run()?;
    let title = run.title.as_deref().unwrap_or(run.workflow());
    Some(match &run.actor {
        Some(a) => format!("{title} — {a}"),
        None => title.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use chrono::{DateTime, TimeZone, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::ci::RunStatus;
    use crate::ci::repo::PagesUpdate;
    use crate::ci::repo::fake::run;
    use crate::provider::types::RepoIssues;
    use crate::tui::harness::HarnessRegistry;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 8, 4, 0, 0).unwrap()
    }

    fn app_with(runs: Vec<CiRun>, total: usize) -> App {
        let mut app = App::new("o".into(), None, false, false, "{number}".into());
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![Issue::fixture("I_1", 1)],
        }]);
        app.runs.supported = true;
        app.selected = 0;
        app.open_detail();
        let t = Instant::now();
        app.repo_runs_follow(t);
        let fetches = app.repo_runs_tick(t + crate::tui::app::runs::DEBOUNCE);
        let g = fetches
            .iter()
            .find_map(|f| match f {
                crate::tui::app::RepoFetch::Pages { generation, .. } => Some(*generation),
                _ => None,
            })
            .unwrap();
        let update = PagesUpdate {
            runs,
            total,
            history: vec![],
        };
        app.apply_repo_pages("r", g, (1, 1), Ok(update), at(), t);
        app
    }

    fn screen(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let reg = HarnessRegistry::default();
        term.draw(|f| crate::tui::ui::draw(f, app, &Theme::default(), &reg))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_selected_repo_header_draws_the_view_in_the_whole_pane() {
        let mut done = run(411, RunStatus::Success);
        done.created_at = Some(at() - TimeDelta::hours(3));
        let app = app_with(vec![run(412, RunStatus::Running), done], 431);
        let s = screen(&app);
        assert!(s.contains("workflows · r"), "{s}");
        assert!(s.contains("group: status (v)"));
        assert!(s.contains("running"));
        assert!(s.contains("CI #412 · main · push"));
        assert!(s.contains("2 of 431"));
        assert!(s.contains("+429 more (L)"));
        assert!(!s.contains("no issue selected"));
    }

    #[test]
    fn the_grouped_by_field_is_left_off_the_run_name() {
        let r = run(9, RunStatus::Success);
        assert_eq!(run_name(&r, Grouping::Status), "CI #9 · main · push");
        assert_eq!(run_name(&r, Grouping::Workflow), "#9 · main · push");
        assert_eq!(run_name(&r, Grouping::Branch), "CI #9 · push");
    }

    #[test]
    fn a_finished_run_says_how_long_ago() {
        assert_eq!(ago(TimeDelta::seconds(20)), "just now");
        assert_eq!(ago(TimeDelta::minutes(5)), "5m ago");
        assert_eq!(ago(TimeDelta::hours(3)), "3h ago");
        assert_eq!(ago(TimeDelta::days(2)), "2d ago");
    }

    #[test]
    fn filters_show_in_the_border() {
        let f = RunFilter {
            status: Some(RunStatus::Failure),
            branch: Some("main".into()),
            ..RunFilter::default()
        };
        assert_eq!(filter_words(&f), vec!["status:failure", "branch:main"]);
    }

    #[test]
    fn the_caption_is_the_selected_runs_title_and_actor() {
        let mut r = run(5, RunStatus::Success);
        r.title = Some("fix: something".into());
        r.actor = Some("pgmac".into());
        let mut app = app_with(vec![r], 1);
        app.repo_runs.group = Grouping::Flat;
        app.focus = Focus::List;
        assert_eq!(caption(&app), None, "only with the pane focused");
        app.focus = Focus::Detail;
        assert_eq!(caption(&app).as_deref(), Some("fix: something — pgmac"));
    }
}
