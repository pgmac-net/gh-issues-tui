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

    let current_repo = app.selected_issue_repo().map(|r| r.repo.as_str());
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

/// What a right-hand figure is, which decides its colour (#240).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fig {
    /// Everything that is not one of the below: conclusion words, `?`,
    /// `no ETA`, PR summaries. Also the separators between figures.
    Dim,
    /// `blocked +1m` and `overdue`: a problem, not a forecast.
    Warning,
    /// Time remaining and the ETA clock: the forecast.
    Eta,
    /// A time taken, live or finished.
    Elapsed,
    /// A done/total count: `1/2` jobs, `3/5` steps.
    Progress,
}

/// One figure on the right of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg {
    fig: Fig,
    text: String,
}

fn seg(fig: Fig, text: impl Into<String>) -> Seg {
    Seg {
        fig,
        text: text.into(),
    }
}

/// Drawn between figures, always dim so the figures stand out from it.
const SEP: &str = " · ";

/// The figures as the text they draw: joined by [`SEP`].
fn flat(figures: &[Seg]) -> String {
    figures
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(SEP)
}

/// The figures as styled spans, with dim separators between them.
fn spans(figures: &[Seg], t: &Theme) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, s) in figures.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(SEP, fig_style(Fig::Dim, t)));
        }
        out.push(Span::styled(s.text.clone(), fig_style(s.fig, t)));
    }
    out
}

fn fig_style(fig: Fig, t: &Theme) -> Style {
    let fg = |c: Color| Style::default().fg(c);
    match fig {
        Fig::Dim => fg(t.dim),
        Fig::Warning => fg(t.warning),
        Fig::Elapsed => fg(t.runs_elapsed),
        Fig::Progress => fg(t.runs_progress),
        Fig::Eta if t.runs_eta_bold => fg(t.runs_eta).add_modifier(Modifier::BOLD),
        Fig::Eta => fg(t.runs_eta),
    }
}

/// A row's text, before fitting it to the width.
struct RowText {
    glyph: &'static str,
    colour: Color,
    name: String,
    right: Vec<Seg>,
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
    let right_w = flat(&text.right).chars().count();
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
    let mut line = vec![
        Span::raw(format!("{indent}{marker}")),
        Span::styled(format!("{} ", text.glyph), Style::default().fg(text.colour)),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
    ];
    line.extend(spans(&text.right, t));
    Line::from(line)
}

fn row_text(data: &RunsData, row: &RunRow, current_repo: Option<&str>, t: &Theme) -> RowText {
    let notice = |name: String, colour| RowText {
        glyph: " ",
        colour,
        name,
        right: Vec::new(),
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
            right: vec![seg(Fig::Dim, "unreadable")],
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
    // Only a forecast is styled as one; counts like `2/3 failed` stay dim,
    // because the glyph already says the PR's state (#240).
    let summary = match (active, failed, longest) {
        (0, 0, _) => seg(Fig::Dim, plural(runs.len(), "run")),
        (0, _, _) => seg(Fig::Dim, format!("{failed}/{} failed", runs.len())),
        (_, _, Some(left)) => seg(Fig::Eta, format!("~{}", fmt_dur(left))),
        (_, _, None) => seg(Fig::Dim, format!("{active}/{} running", runs.len())),
    };
    let right = if pr.merged() {
        vec![seg(Fig::Dim, "merged"), summary]
    } else {
        vec![summary]
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
            let mut right = vec![
                seg(Fig::Progress, format!("{done}/{}", run.jobs.len())),
                seg(Fig::Elapsed, fmt_dur(e.elapsed)),
            ];
            right.extend(eta_figures(e));
            right
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

/// The run's forecast, in gh-eta's vocabulary: remaining time and the ETA
/// clock are forecasts (`Eta`); `blocked` is a problem (`Warning`); having no
/// history to forecast from is neither (`Dim`).
fn eta_figures(e: &RunEstimate) -> Vec<Seg> {
    if e.history_runs == 0 {
        return vec![seg(Fig::Dim, "no ETA")];
    }
    if e.blocked {
        return vec![seg(
            Fig::Warning,
            match e.remaining > TimeDelta::zero() {
                true => format!("blocked +{}", fmt_dur(e.remaining)),
                false => "blocked".into(),
            },
        )];
    }
    let partial = if e.partial { "+" } else { "" };
    vec![
        seg(Fig::Eta, format!("~{}{partial}", fmt_dur(e.remaining))),
        seg(
            Fig::Eta,
            format!("ETA {}", crate::localtime::fmt_time(e.eta)),
        ),
    ]
}

/// A finished item's figures: its conclusion unless the glyph already says all
/// there is, then how long it took.
fn finished(conclusion: Option<&str>, took: &str) -> Vec<Seg> {
    let took = (!took.is_empty()).then(|| seg(Fig::Elapsed, took));
    match conclusion {
        Some("success") | None => took.into_iter().collect(),
        Some(c) => std::iter::once(seg(Fig::Dim, c)).chain(took).collect(),
    }
}

fn job_text(
    job: &ActionsJob,
    estimate: Option<&JobEstimate>,
    data: &RunsData,
    t: &Theme,
) -> RowText {
    let (glyph, colour) = status_mark(&job.status, job.conclusion.as_deref(), t);
    let elapsed = |e: &JobEstimate| seg(Fig::Elapsed, fmt_dur(e.elapsed));
    let right = match estimate.map(|e| (e, e.state)) {
        Some((e, JobState::Running)) => {
            let mut right = Vec::new();
            if !job.steps.is_empty() {
                let done = job.steps.iter().filter(|s| s.status == "completed").count();
                right.push(seg(Fig::Progress, format!("{done}/{}", job.steps.len())));
            }
            right.push(elapsed(e));
            right.push(seg(Fig::Eta, format!("~{}", fmt_dur(e.remaining))));
            right
        }
        Some((e, JobState::Overdue)) => vec![elapsed(e), seg(Fig::Warning, "overdue")],
        Some((e, JobState::Unknown)) if job.status == "in_progress" => {
            vec![elapsed(e), seg(Fig::Dim, "?")]
        }
        Some((_, JobState::Unknown)) => vec![seg(Fig::Dim, "?")],
        Some((e, JobState::Blocked)) => {
            vec![seg(
                Fig::Warning,
                format!("blocked +{}", fmt_dur(e.remaining)),
            )]
        }
        Some((e, JobState::Pending)) => {
            vec![seg(Fig::Eta, format!("~{}", fmt_dur(e.remaining)))]
        }
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
) -> Vec<Seg> {
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
        assert_eq!(flat(&eta_figures(&e)), "no ETA");
        e.history_runs = 3;
        e.remaining = TimeDelta::seconds(90);
        e.eta = at() + e.remaining;
        let time = crate::localtime::fmt_time(e.eta);
        assert_eq!(flat(&eta_figures(&e)), format!("~1m30s · ETA {time}"));
        e.partial = true;
        assert_eq!(flat(&eta_figures(&e)), format!("~1m30s+ · ETA {time}"));
        e.blocked = true;
        assert_eq!(flat(&eta_figures(&e)), "blocked +1m30s");
    }

    #[test]
    fn a_failed_job_says_how_and_a_good_one_only_how_long() {
        assert_eq!(flat(&finished(Some("success"), "12s")), "12s");
        assert_eq!(flat(&finished(Some("timed_out"), "12s")), "timed_out · 12s");
        assert_eq!(flat(&finished(Some("skipped"), "")), "skipped");
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
            flat(&job_text(&running, Some(&estimate), &data, &t).right),
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
            flat(&job_text(&running, Some(&estimate), &data, &t).right),
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
            flat(&job_text(&running, Some(&estimate), &data, &t).right),
            "10m00s · overdue"
        );
        estimate.state = JobState::Unknown;
        assert_eq!(
            flat(&job_text(&running, Some(&estimate), &data, &t).right),
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
        assert_eq!(flat(&text.right), "30s");
    }

    /// The style of the first span whose text is exactly `text`.
    fn span_style(spans: &[Span<'static>], text: &str) -> Style {
        spans
            .iter()
            .find(|s| s.content == text)
            .unwrap_or_else(|| panic!("no span {text:?} in {spans:?}"))
            .style
    }

    fn fg(c: Color) -> Style {
        Style::default().fg(c)
    }

    /// A running run with one of two jobs done, 1m58s in, 1m30s to go.
    fn forecast_run() -> (ActionsRun, RunEstimate) {
        let r = run(
            10,
            "in_progress",
            None,
            vec![
                job(100, "fmt", "completed", Some("success")),
                job(101, "test", "in_progress", None),
            ],
        );
        let mut e = forecast(&r, &Default::default(), at());
        e.history_runs = 3;
        e.partial = false;
        e.blocked = false;
        e.elapsed = TimeDelta::seconds(118);
        e.remaining = TimeDelta::seconds(90);
        e.eta = at() + e.remaining;
        (r, e)
    }

    /// #240: progress, time taken and forecast are three different colours,
    /// the forecast alone is bold, and the separators stay dim.
    #[test]
    fn a_run_row_colours_progress_elapsed_and_forecast_apart() {
        let t = Theme::default();
        let (r, e) = forecast_run();
        let time = crate::localtime::fmt_time(e.eta);
        let sp = spans(&run_text(&r, Some(&e), &t).right, &t);

        let eta = fg(t.runs_eta).add_modifier(Modifier::BOLD);
        assert_eq!(span_style(&sp, "1/2"), fg(t.runs_progress));
        assert_eq!(span_style(&sp, "1m58s"), fg(t.runs_elapsed));
        assert_eq!(span_style(&sp, "~1m30s"), eta);
        assert_eq!(span_style(&sp, &format!("ETA {time}")), eta);
        assert_eq!(span_style(&sp, " · "), fg(t.dim));
        assert_ne!(t.runs_progress, t.runs_elapsed);
        assert_ne!(t.runs_eta, t.runs_elapsed);
    }

    #[test]
    fn the_default_elapsed_colour_is_the_one_it_always_had() {
        let t = Theme::default();
        assert_eq!(t.runs_elapsed, t.dim);
        let (r, e) = forecast_run();
        let sp = spans(&run_text(&r, Some(&e), &t).right, &t);
        assert_eq!(span_style(&sp, "1m58s"), fg(t.dim));
    }

    /// #240: every one of the three colours, and the bold, is configurable.
    #[test]
    fn a_profile_recolours_the_figures_and_can_turn_the_bold_off() {
        let profile: crate::tui::theme::ColorProfile = toml::from_str(
            "runs_eta      = \"#ff0000\"\n\
             runs_elapsed  = \"#00ff00\"\n\
             runs_progress = \"#0000ff\"\n\
             runs_eta_bold = false\n",
        )
        .unwrap();
        let t = Theme::with_profile(&profile);
        let (r, e) = forecast_run();
        let sp = spans(&run_text(&r, Some(&e), &t).right, &t);
        assert_eq!(span_style(&sp, "1/2"), fg(Color::Rgb(0, 0, 255)));
        assert_eq!(span_style(&sp, "1m58s"), fg(Color::Rgb(0, 255, 0)));
        assert_eq!(
            span_style(&sp, "~1m30s"),
            fg(Color::Rgb(255, 0, 0)),
            "no bold when runs_eta_bold is false"
        );
    }

    #[test]
    fn problems_are_warnings_and_the_absence_of_a_forecast_is_dim() {
        let t = Theme::default();
        let (_, mut e) = forecast_run();
        let r = run(10, "in_progress", None, vec![]);

        e.blocked = true;
        let sp = spans(&run_text(&r, Some(&e), &t).right, &t);
        assert_eq!(span_style(&sp, "blocked +1m30s"), fg(t.warning));

        e.blocked = false;
        e.history_runs = 0;
        let sp = spans(&run_text(&r, Some(&e), &t).right, &t);
        assert_eq!(span_style(&sp, "no ETA"), fg(t.dim));

        let data = RunsData::default();
        let running = job(1, "test", "in_progress", None);
        let overdue = JobEstimate {
            state: JobState::Overdue,
            elapsed: TimeDelta::seconds(600),
            remaining: TimeDelta::zero(),
            basis: None,
        };
        let sp = spans(&job_text(&running, Some(&overdue), &data, &t).right, &t);
        assert_eq!(span_style(&sp, "10m00s"), fg(t.runs_elapsed));
        assert_eq!(span_style(&sp, "overdue"), fg(t.warning));
        let unknown = JobEstimate {
            state: JobState::Unknown,
            ..overdue
        };
        let sp = spans(&job_text(&running, Some(&unknown), &data, &t).right, &t);
        assert_eq!(span_style(&sp, "?"), fg(t.dim));
    }

    #[test]
    fn a_job_row_colours_its_step_count_and_a_finished_one_its_time_taken() {
        let t = Theme::default();
        let data = RunsData::default();
        let mut running = job(1, "test", "in_progress", None);
        running.steps = vec![
            ActionsStep {
                name: "a".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                started_at: None,
                completed_at: None,
            },
            ActionsStep {
                name: "b".into(),
                status: "in_progress".into(),
                conclusion: None,
                started_at: None,
                completed_at: None,
            },
        ];
        let estimate = JobEstimate {
            state: JobState::Running,
            elapsed: TimeDelta::seconds(118),
            remaining: TimeDelta::seconds(90),
            basis: Some(Basis::Steps),
        };
        let sp = spans(&job_text(&running, Some(&estimate), &data, &t).right, &t);
        assert_eq!(span_style(&sp, "1/2"), fg(t.runs_progress));
        assert_eq!(
            span_style(&sp, "~1m30s"),
            fg(t.runs_eta).add_modifier(Modifier::BOLD)
        );

        let sp = spans(&finished(Some("timed_out"), "12s"), &t);
        assert_eq!(
            span_style(&sp, "timed_out"),
            fg(t.dim),
            "conclusion words stay dim"
        );
        assert_eq!(span_style(&sp, "12s"), fg(t.runs_elapsed));
    }

    /// A PR row's summary is a count, not a forecast, unless it is the
    /// remaining time of its longest run.
    #[test]
    fn a_pr_summary_stays_dim_unless_it_is_a_forecast() {
        let t = Theme::default();
        let failed = PrRuns {
            pr: linked(301, "r"),
            runs: Ok(vec![run(10, "completed", Some("failure"), vec![])]),
        };
        let sp = spans(&pr_text(&failed, &RunsData::default(), None, &t).right, &t);
        assert_eq!(span_style(&sp, "1/1 failed"), fg(t.dim));

        let active = PrRuns {
            pr: linked(301, "r"),
            runs: Ok(vec![run(10, "in_progress", None, vec![])]),
        };
        let (_, e) = forecast_run();
        let mut data = RunsData::default();
        data.estimates.insert(10, e);
        let sp = spans(&pr_text(&active, &data, None, &t).right, &t);
        assert_eq!(
            span_style(&sp, "~1m30s"),
            fg(t.runs_eta).add_modifier(Modifier::BOLD)
        );
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
