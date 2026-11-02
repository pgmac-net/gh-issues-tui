//! The workflow-runs tab of the detail pane (#222).
//!
//! One row per tree row, never wrapped: a name on the left, truncated, and its
//! figures on the right, always whole. That keeps the tree scannable and makes
//! the row count the region's height.

use chrono::TimeDelta;
use unicode_width::UnicodeWidthChar;

use super::detail::Highlight;
use super::list::pane_border;
use super::prelude::*;
use super::widgets::render_region_scrollbar;
use crate::runs::estimate::{JobEstimate, JobState, RunEstimate};
use crate::runs::fetch::PrRuns;
use crate::runs::{ActionsJob, ActionsRun, ActionsStep, fmt_dur, is_failure};
use crate::tui::app::{DetailTab, Focus, RunRow, RunRowKind, RunsData};

/// The lower region's title: which tab is showing, and the key that flips it.
/// A provider without runs keeps the plain title it always had.
pub(super) fn tab_title(app: &App, t: &Theme) -> Line<'static> {
    if !app.runs.supported {
        return Line::from(" comments ");
    }
    let style = |tab| {
        if app.runs.tab == tab {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.dim)
        }
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled("comments", style(DetailTab::Comments)),
        Span::raw(" │ "),
        Span::styled("runs", style(DetailTab::Runs)),
        Span::styled(" (R) ", Style::default().fg(t.dim)),
    ])
}

pub(super) fn draw_detail_runs(f: &mut Frame, app: &App, t: &Theme, area: Rect, focused: bool) {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border(app, t, Focus::Detail))
        .title(tab_title(app, t));
    if let Some(note) = footnote(app) {
        block = block.title_bottom(Line::styled(note, Style::default().fg(t.warning)));
    }
    let message = |text: String, colour: Color| Line::styled(text, Style::default().fg(colour));

    let data = match &app.runs.data {
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
    if data.prs.is_empty() {
        let text = message("no linked PRs".into(), t.dim);
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let rows = app.runs.rows();
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let sel = app.runs.sel.min(rows.len().saturating_sub(1));
    let top = layout::scroll_top(app.runs.top.get(), sel, inner_h, rows.len());
    app.runs.top.set(top);

    let current_repo = app.selected_repo().map(|r| r.repo.as_str());
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(inner_h)
        .map(|(i, row)| {
            let hl = Highlight::of(i == sel, focused);
            row_line(data, row, current_repo, hl, inner_w, t)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
    let total = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let scroll = u16::try_from(top).unwrap_or(u16::MAX);
    render_region_scrollbar(f, t, area, total, inner_h as u16, scroll);
}

/// Why the rows may be behind, in the bottom border so it costs no row.
fn footnote(app: &App) -> Option<String> {
    if app.runs.paused {
        return Some(" polling paused: REST budget low ".into());
    }
    app.runs
        .stale
        .as_ref()
        .map(|e| format!(" refresh failed: {e} "))
}

/// A row's text, before fitting it to the width.
struct RowText {
    glyph: &'static str,
    colour: Color,
    name: String,
    right: String,
}

fn row_line(
    data: &RunsData,
    row: &RunRow,
    current_repo: Option<&str>,
    hl: Highlight,
    width: usize,
    t: &Theme,
) -> Line<'static> {
    let text = row_text(data, row, current_repo, t);
    let indent = "  ".repeat(row.depth as usize);
    let marker = match (row.expandable, row.expanded) {
        (false, _) => "  ",
        (true, true) => "▾ ",
        (true, false) => "▸ ",
    };
    let right_w = text.right.chars().count();
    let used = indent.len() + 2 + text.glyph.chars().count() + 1;
    let name_w = width.saturating_sub(used + right_w + 1);
    let name = fit(&text.name, name_w);
    let pad = width.saturating_sub(used + name.width() + right_w);

    let mut name_style = Style::default();
    match hl {
        Highlight::None => {}
        Highlight::Focused => name_style = name_style.add_modifier(Modifier::REVERSED),
        Highlight::Unfocused => name_style = name_style.bg(t.selected_bg_unfocused),
    }
    Line::from(vec![
        Span::raw(format!("{indent}{marker}")),
        Span::styled(format!("{} ", text.glyph), Style::default().fg(text.colour)),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(text.right, Style::default().fg(t.dim)),
    ])
}

fn row_text(data: &RunsData, row: &RunRow, current_repo: Option<&str>, t: &Theme) -> RowText {
    let notice = |name: String, colour| RowText {
        glyph: " ",
        colour,
        name,
        right: String::new(),
    };
    let runs_of = |pr: usize| data.prs[pr].runs.as_deref().unwrap_or(&[]);
    match row.kind {
        RunRowKind::Pr { pr } => pr_text(&data.prs[pr], data, current_repo, t),
        RunRowKind::Run { pr, run } => {
            let run = &runs_of(pr)[run];
            run_text(run, data.estimates.get(&run.id), t)
        }
        RunRowKind::Job { pr, run, job } => {
            let run = &runs_of(pr)[run];
            let estimate = data.estimates.get(&run.id).and_then(|e| e.jobs.get(job));
            job_text(&run.jobs[job], estimate, data, t)
        }
        RunRowKind::Step { pr, run, job, step } => {
            step_text(&runs_of(pr)[run].jobs[job].steps[step], data, t)
        }
        RunRowKind::Failed { pr } => notice(
            data.prs[pr]
                .runs
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_default(),
            t.error,
        ),
        RunRowKind::NoRuns => notice("no runs for this commit".into(), t.dim),
        RunRowKind::More(n) => notice(format!("+{n} more"), t.dim),
    }
}

/// The glyph and colour for an Actions status and conclusion.
fn status_mark(status: &str, conclusion: Option<&str>, t: &Theme) -> (&'static str, Color) {
    match status {
        "completed" if is_failure(conclusion) => ("✗", t.error),
        "completed" if conclusion == Some("success") => ("✓", t.open),
        "completed" => ("-", t.dim),
        "in_progress" => ("●", t.accent),
        "waiting" => ("!", t.warning),
        _ => ("·", t.dim),
    }
}

fn pr_text(entry: &PrRuns, data: &RunsData, current_repo: Option<&str>, t: &Theme) -> RowText {
    let pr = &entry.pr;
    let name = if current_repo == Some(pr.pr.repo.as_str()) {
        format!("#{} {}", pr.pr.number, pr.title)
    } else {
        format!("{}#{} {}", pr.pr.repo, pr.pr.number, pr.title)
    };
    let Ok(runs) = &entry.runs else {
        return RowText {
            glyph: "✗",
            colour: t.error,
            name,
            right: "unreadable".into(),
        };
    };
    let active = runs.iter().filter(|r| r.is_active()).count();
    let failed = runs.iter().filter(|r| r.is_failed()).count();
    let (glyph, colour) = if active > 0 {
        ("●", t.accent)
    } else if failed > 0 {
        ("✗", t.error)
    } else if runs.is_empty() {
        ("·", t.dim)
    } else {
        ("✓", t.open)
    };
    let longest = runs
        .iter()
        .filter_map(|r| data.estimates.get(&r.id))
        .filter(|e| e.history_runs > 0)
        .map(|e| e.remaining)
        .max();
    let summary = match (active, failed, longest) {
        (0, 0, _) => plural(runs.len(), "run"),
        (0, _, _) => format!("{failed}/{} failed", runs.len()),
        (_, _, Some(left)) => format!("~{}", fmt_dur(left)),
        (_, _, None) => format!("{active}/{} running", runs.len()),
    };
    let right = if pr.merged() {
        format!("merged · {summary}")
    } else {
        summary
    };
    RowText {
        glyph,
        colour,
        name,
        right,
    }
}

fn plural(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

fn run_text(run: &ActionsRun, estimate: Option<&RunEstimate>, t: &Theme) -> RowText {
    let (glyph, colour) = status_mark(&run.status, run.conclusion.as_deref(), t);
    let name = format!("{} · {}", run.workflow(), run.event);
    let right = match estimate {
        Some(e) => {
            let done = run.jobs.iter().filter(|j| j.status == "completed").count();
            let progress = format!("{done}/{}", run.jobs.len());
            format!("{progress} · {} · {}", fmt_dur(e.elapsed), eta_text(e))
        }
        None => {
            let took = run
                .run_started_at
                .zip(run.updated_at)
                .map(|(start, end)| fmt_dur(end - start))
                .unwrap_or_default();
            finished(run.conclusion.as_deref(), &took)
        }
    };
    RowText {
        glyph,
        colour,
        name,
        right,
    }
}

/// The run's forecast, in gh-eta's vocabulary.
fn eta_text(e: &RunEstimate) -> String {
    if e.history_runs == 0 {
        return "no ETA".into();
    }
    if e.blocked {
        return match e.remaining > TimeDelta::zero() {
            true => format!("blocked +{}", fmt_dur(e.remaining)),
            false => "blocked".into(),
        };
    }
    let partial = if e.partial { "+" } else { "" };
    format!(
        "~{}{partial} · ETA {}",
        fmt_dur(e.remaining),
        crate::localtime::fmt_time(e.eta)
    )
}

/// A finished item's figures: how long it took, and its conclusion unless the
/// glyph already says all there is.
fn finished(conclusion: Option<&str>, took: &str) -> String {
    match conclusion {
        Some("success") | None => took.to_string(),
        Some(c) if took.is_empty() => c.to_string(),
        Some(c) => format!("{c} · {took}"),
    }
}

fn job_text(
    job: &ActionsJob,
    estimate: Option<&JobEstimate>,
    data: &RunsData,
    t: &Theme,
) -> RowText {
    let (glyph, colour) = status_mark(&job.status, job.conclusion.as_deref(), t);
    let right = match estimate.map(|e| (e, e.state)) {
        Some((e, JobState::Running)) => {
            let steps = if !job.steps.is_empty() {
                let done = job.steps.iter().filter(|s| s.status == "completed").count();
                format!("{done}/{} · ", job.steps.len())
            } else {
                String::new()
            };
            format!("{steps}{} · ~{}", fmt_dur(e.elapsed), fmt_dur(e.remaining))
        }
        Some((e, JobState::Overdue)) => format!("{} · overdue", fmt_dur(e.elapsed)),
        Some((e, JobState::Unknown)) if job.status == "in_progress" => {
            format!("{} · ?", fmt_dur(e.elapsed))
        }
        Some((_, JobState::Unknown)) => "?".into(),
        Some((e, JobState::Blocked)) => format!("blocked +{}", fmt_dur(e.remaining)),
        Some((e, JobState::Pending)) => format!("~{}", fmt_dur(e.remaining)),
        Some((_, JobState::Done)) | None => times(
            job.started_at,
            job.completed_at,
            job.conclusion.as_deref(),
            data,
        ),
    };
    RowText {
        glyph,
        colour,
        name: job.name.clone(),
        right,
    }
}

fn step_text(step: &ActionsStep, data: &RunsData, t: &Theme) -> RowText {
    let (glyph, colour) = status_mark(&step.status, step.conclusion.as_deref(), t);
    RowText {
        glyph,
        colour,
        name: step.name.clone(),
        right: times(
            step.started_at,
            step.completed_at,
            step.conclusion.as_deref(),
            data,
        ),
    }
}

/// A job or step's own timing. Something unfinished has run until the data
/// landed; something unstarted has nothing to show.
fn times(
    started: Option<chrono::DateTime<chrono::Utc>>,
    completed: Option<chrono::DateTime<chrono::Utc>>,
    conclusion: Option<&str>,
    data: &RunsData,
) -> String {
    let took = started
        .map(|start| fmt_dur(completed.unwrap_or(data.as_of) - start))
        .unwrap_or_default();
    finished(conclusion, &took)
}

/// Cut `s` to `width` terminal columns, ending in `…` if anything went.
fn fit(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let Some(room) = width.checked_sub(1) else {
        return String::new();
    };
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use chrono::{DateTime, TimeZone, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::provider::types::{PrRef, RepoIssues};
    use crate::runs::LinkedPr;
    use crate::runs::estimate::{Basis, estimate as forecast};
    use crate::runs::fetch::RunsUpdate;
    use crate::tui::harness::HarnessRegistry;
    use crate::tui::ui;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 17, 4, 0, 0).unwrap()
    }

    fn issue() -> Issue {
        Issue {
            title: "an issue".into(),
            created_at: at(),
            updated_at: at(),
            ..Issue::fixture("I_1", 1)
        }
    }

    fn app() -> App {
        let mut app = App::new("o".into(), None, false, false, "{number}".into());
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "https://github.com/o/r".into(),
            issues: vec![issue()],
        }]);
        app.selected = 1;
        app.runs.supported = true;
        app.open_detail();
        app.detail.comments = Some(vec![]);
        app
    }

    fn linked(number: u64, repo: &str) -> LinkedPr {
        LinkedPr {
            pr: PrRef {
                owner: "o".into(),
                repo: repo.into(),
                number,
            },
            title: "feat: runs pane".into(),
            url: format!("https://github.com/o/{repo}/pull/{number}"),
            head_sha: "sha".into(),
            open: true,
            merged_at: None,
        }
    }

    fn job(id: i64, name: &str, status: &str, conclusion: Option<&str>) -> ActionsJob {
        ActionsJob {
            id,
            name: name.into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            started_at: Some(at()),
            completed_at: conclusion.map(|_| at() + TimeDelta::seconds(12)),
            html_url: None,
            steps: vec![],
        }
    }

    fn run(id: i64, status: &str, conclusion: Option<&str>, jobs: Vec<ActionsJob>) -> ActionsRun {
        ActionsRun {
            id,
            workflow_id: 1,
            name: Some("CI".into()),
            event: "pull_request".into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            run_number: 1,
            run_started_at: Some(at()),
            updated_at: Some(at() + TimeDelta::seconds(150)),
            html_url: "u".into(),
            jobs,
        }
    }

    /// The whole frame, rendered with the runs tab showing `prs`.
    fn frame(prs: Vec<PrRuns>) -> String {
        let mut app = app();
        let t = Instant::now();
        app.toggle_runs_tab(t);
        let fetch = app.runs_tick(t).expect("a load");
        let update = RunsUpdate {
            prs,
            hidden: 0,
            history: vec![],
        };
        app.apply_runs("I_1", fetch.generation, Ok(update), at(), t);
        rendered(&app)
    }

    fn rendered(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| ui::draw(f, app, &Theme::default(), &HarnessRegistry::default()))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_tree_shows_workflow_job_and_progress() {
        let text = frame(vec![PrRuns {
            pr: linked(301, "r"),
            runs: Ok(vec![run(
                10,
                "in_progress",
                None,
                vec![
                    job(100, "fmt", "completed", Some("success")),
                    job(101, "test", "in_progress", None),
                ],
            )]),
        }]);
        assert!(text.contains("#301 feat: runs pane"), "{text}");
        assert!(text.contains("CI · pull_request"), "{text}");
        assert!(text.contains("1/2"), "job progress: {text}");
        assert!(text.contains("fmt"), "{text}");
        assert!(text.contains("no ETA"), "no history means no ETA: {text}");
    }

    #[test]
    fn a_pr_in_another_repo_names_it() {
        let text = frame(vec![PrRuns {
            pr: linked(9, "other"),
            runs: Ok(vec![]),
        }]);
        assert!(text.contains("other#9"), "{text}");
    }

    #[test]
    fn no_linked_prs_says_so() {
        assert!(frame(vec![]).contains("no linked PRs"));
    }

    #[test]
    fn a_pr_that_cannot_be_read_shows_why() {
        let text = frame(vec![PrRuns {
            pr: linked(9, "r"),
            runs: Err("HTTP 403 reading Actions".into()),
        }]);
        assert!(text.contains("HTTP 403 reading Actions"), "{text}");
    }

    #[test]
    fn the_loading_state_shows_before_anything_lands() {
        let mut app = app();
        app.toggle_runs_tab(Instant::now());
        assert!(rendered(&app).contains("loading runs…"));
    }

    #[test]
    fn a_failed_load_shows_the_error() {
        let mut app = app();
        let t = Instant::now();
        app.toggle_runs_tab(t);
        let fetch = app.runs_tick(t).unwrap();
        app.apply_runs("I_1", fetch.generation, Err("boom".into()), at(), t);
        assert!(rendered(&app).contains("runs failed: boom"));
    }

    #[test]
    fn the_comments_tab_names_both_tabs_only_with_a_provider_that_has_runs() {
        let mut app = app();
        assert!(rendered(&app).contains("comments │ runs (R)"));
        app.runs.supported = false;
        let text = rendered(&app);
        assert!(text.contains(" comments "), "{text}");
        assert!(!text.contains("runs (R)"), "{text}");
    }

    #[test]
    fn eta_text_uses_gh_etas_vocabulary() {
        let mut e = forecast(
            &run(1, "in_progress", None, vec![]),
            &Default::default(),
            at(),
        );
        assert_eq!(eta_text(&e), "no ETA");
        e.history_runs = 3;
        e.remaining = TimeDelta::seconds(90);
        e.eta = at() + e.remaining;
        let time = crate::localtime::fmt_time(e.eta);
        assert_eq!(eta_text(&e), format!("~1m30s · ETA {time}"));
        e.partial = true;
        assert_eq!(eta_text(&e), format!("~1m30s+ · ETA {time}"));
        e.blocked = true;
        assert_eq!(eta_text(&e), "blocked +1m30s");
    }

    #[test]
    fn a_failed_job_says_how_and_a_good_one_only_how_long() {
        assert_eq!(finished(Some("success"), "12s"), "12s");
        assert_eq!(finished(Some("timed_out"), "12s"), "timed_out · 12s");
        assert_eq!(finished(Some("skipped"), ""), "skipped");
    }

    #[test]
    fn a_running_job_shows_its_remaining_time_and_steps_done() {
        let data = RunsData::default();
        let t = Theme::default();
        let estimate = JobEstimate {
            state: JobState::Running,
            elapsed: TimeDelta::seconds(118),
            remaining: TimeDelta::seconds(90),
            basis: Some(Basis::Steps),
        };
        let mut running = job(1, "test", "in_progress", None);
        assert_eq!(
            job_text(&running, Some(&estimate), &data, &t).right,
            "1m58s · ~1m30s",
            "no steps reported yet"
        );
        let step = |status: &str, conclusion: Option<&str>| ActionsStep {
            name: "s".into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            started_at: None,
            completed_at: None,
        };
        running.steps = vec![
            step("completed", Some("success")),
            step("in_progress", None),
            step("queued", None),
        ];
        assert_eq!(
            job_text(&running, Some(&estimate), &data, &t).right,
            "1/3 · 1m58s · ~1m30s"
        );
    }

    #[test]
    fn overdue_and_unknown_jobs_say_so() {
        let data = RunsData::default();
        let t = Theme::default();
        let mut estimate = JobEstimate {
            state: JobState::Overdue,
            elapsed: TimeDelta::seconds(600),
            remaining: TimeDelta::zero(),
            basis: None,
        };
        let running = job(1, "test", "in_progress", None);
        assert_eq!(
            job_text(&running, Some(&estimate), &data, &t).right,
            "10m00s · overdue"
        );
        estimate.state = JobState::Unknown;
        assert_eq!(
            job_text(&running, Some(&estimate), &data, &t).right,
            "10m00s · ?"
        );
    }

    #[test]
    fn a_job_with_no_estimate_has_run_until_the_data_landed() {
        let data = RunsData {
            as_of: at() + TimeDelta::seconds(30),
            ..RunsData::default()
        };
        let text = job_text(
            &job(1, "test", "in_progress", None),
            None,
            &data,
            &Theme::default(),
        );
        assert_eq!(text.right, "30s");
    }

    #[test]
    fn fit_truncates_by_columns() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a long name", 6), "a lon…");
        assert_eq!(fit("日本語", 4), "日…");
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn the_scroll_keeps_the_cursor_on_screen() {
        let prs = vec![PrRuns {
            pr: linked(1, "r"),
            runs: Ok((0..40)
                .map(|i| run(i, "in_progress", None, vec![]))
                .collect()),
        }];
        let mut app = app();
        let t = Instant::now();
        app.toggle_runs_tab(t);
        let fetch = app.runs_tick(t).unwrap();
        let update = RunsUpdate {
            prs,
            hidden: 0,
            history: vec![],
        };
        app.apply_runs("I_1", fetch.generation, Ok(update), at(), t);
        app.runs_jump_end();
        rendered(&app);
        assert!(app.runs.top.get() > 0, "scrolled down to the cursor");
    }
}
