//! The workflow-runs tab (#222): what it shows, when it may ask for more, and
//! the tree the cursor walks.
//!
//! Scheduling lives here as data so it can be tested without a clock or a task:
//! `due` says when the event loop should call [`App::runs_tick`], which decides
//! whether anything is actually sent. One timer covers the debounce and the
//! poll, and the only thing that re-arms a poll is a landed response, so two
//! fetches for one issue can never overlap.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::prelude::*;
use super::run_tree::{RunNode, RunRow, RunsData};
use crate::runs::HistoryKey;
use crate::runs::estimate::{History, RunEstimate, estimate};
use crate::runs::fetch::{Known, PrRuns, RunsUpdate};

/// How often a tab showing an active run re-reads it. gh-eta's default.
pub const POLL: Duration = Duration::from_secs(15);
/// How long the selection must settle before a load: holding `j` in the list
/// must not cost a load per row.
pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// How long a load waits between checks for the comment thread, whose text may
/// hold PR links.
const COMMENT_WAIT: Duration = Duration::from_millis(250);
/// Checks before loading without the thread; a failed comment fetch leaves it
/// unset for good.
const MAX_COMMENT_WAITS: u8 = 8;
/// REST budget below which polling stops, leaving room for everything else.
pub const RATE_FLOOR: u64 = 100;

/// What the lower detail region shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Comments,
    Runs,
}

/// One fetch for the event loop to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsFetch {
    pub issue_id: String,
    pub generation: u64,
    pub work: RunsWork,
    pub known: Known,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunsWork {
    /// Find the issue's PRs from scratch.
    Load { text_refs: Vec<PrRef> },
    /// Re-read the PRs already known.
    Poll { prs: Vec<PrRuns>, hidden: usize },
}

/// The tab's state. See the module docs for how `due` works.
#[derive(Debug, Default)]
pub struct RunsState {
    /// Whether the provider has runs at all. Set once at startup; no reset
    /// touches it.
    pub supported: bool,
    pub tab: DetailTab,
    /// The issue the tab is about.
    pub target: Option<String>,
    /// `None` until the first answer for `target`.
    pub data: Option<Result<RunsData, String>>,
    /// When the event loop should call [`App::runs_tick`].
    pub due: Option<Instant>,
    /// Only ever increases, so an overtaken response can always be recognised.
    /// No reset touches it.
    generation: u64,
    in_flight: bool,
    /// The next fetch must find PRs from scratch.
    want_load: bool,
    comment_waits: u8,
    /// A refresh failed; `data` is the last good answer.
    pub stale: Option<String>,
    /// Polling is held back by the REST budget.
    pub paused: bool,
    /// Most recently observed REST budget.
    pub budget: Option<RateLimitData>,
    /// Row cursor into [`RunsData::rows`].
    pub sel: usize,
    /// First visible row, remembered between frames (see `layout::scroll_top`).
    pub top: Cell<usize>,
    /// User fold choices; absent nodes take the default for their status.
    pub folds: HashMap<RunNode, bool>,
    /// ETA history by workflow and event. Session-only: a poll must not cost
    /// about 11 requests per workflow, and history barely moves in a session.
    pub history: HashMap<HistoryKey, Arc<History>>,
}

impl RunsState {
    /// Point at a new issue (or none): everything about the old one goes, and
    /// any response still out for it is overtaken.
    fn retarget(&mut self, target: Option<String>) {
        self.target = target;
        self.data = None;
        self.stale = None;
        self.paused = false;
        self.sel = 0;
        self.top.set(0);
        self.in_flight = false;
        self.want_load = true;
        self.comment_waits = 0;
        self.due = None;
        self.generation += 1;
    }

    /// The detail pane closed or opened: back to the comments tab, nothing
    /// outstanding. Keeps `history` and `folds`, which outlive any one view.
    pub fn close(&mut self) {
        self.tab = DetailTab::Comments;
        self.retarget(None);
    }

    /// The org changed: as [`RunsState::close`], and what was learned about
    /// the old org goes too.
    pub fn reset(&mut self) {
        self.close();
        self.folds.clear();
        self.history.clear();
    }

    pub fn rows(&self) -> Vec<RunRow> {
        match &self.data {
            Some(Ok(data)) => data.rows(&self.folds),
            _ => Vec::new(),
        }
    }

    fn known(&self) -> Known {
        let finished_jobs = match &self.data {
            Some(Ok(data)) => data
                .prs
                .iter()
                .filter_map(|p| p.runs.as_ref().ok())
                .flatten()
                .filter(|r| !r.is_active() && !r.jobs.is_empty())
                .map(|r| (r.id, r.jobs.clone()))
                .collect(),
            _ => HashMap::new(),
        };
        Known {
            finished_jobs,
            history: self.history.keys().cloned().collect(),
        }
    }

    fn throttled(&self) -> bool {
        self.budget
            .is_some_and(|b| b.remaining < RATE_FLOOR && b.reset > Utc::now().timestamp())
    }

    pub(super) fn selected_node(&self) -> Option<RunNode> {
        self.rows().get(self.sel).and_then(|r| r.node.clone())
    }

    /// Put the cursor back on `node` after the rows changed, or clamp it.
    pub(super) fn reselect(&mut self, node: Option<RunNode>) {
        let rows = self.rows();
        let found = node.and_then(|n| rows.iter().position(|r| r.node.as_ref() == Some(&n)));
        self.sel = found.unwrap_or(self.sel).min(rows.len().saturating_sub(1));
    }
}

impl App {
    /// Whether the runs tab is what the lower detail region is showing.
    pub fn runs_showing(&self) -> bool {
        self.detail.open && self.runs.supported && self.runs.tab == DetailTab::Runs
    }

    /// `R`: flip the lower detail region between comments and runs.
    pub fn toggle_runs_tab(&mut self, now: Instant) {
        self.runs.tab = match self.runs.tab {
            DetailTab::Comments => DetailTab::Runs,
            DetailTab::Runs => DetailTab::Comments,
        };
        if self.runs_showing() {
            self.runs_follow_selection(now);
        } else {
            self.runs.due = None;
        }
    }

    /// Bring the tab in line with the selected issue: a new issue loads after
    /// the debounce, the same issue picks its poll back up if it needs one.
    pub fn runs_follow_selection(&mut self, now: Instant) {
        let current = self.selected_issue().map(|i| i.id.clone());
        if current != self.runs.target {
            self.runs.retarget(current);
            if self.runs.target.is_some() {
                self.runs.due = Some(now + DEBOUNCE);
            }
            return;
        }
        if self.runs.in_flight || self.runs.target.is_none() {
            return;
        }
        let stale = match &self.runs.data {
            Some(Ok(data)) => data.has_active_run(),
            Some(Err(_)) | None => true,
        };
        if stale {
            self.runs.due = Some(now);
        }
    }

    /// `r` on the tab: find PRs from scratch, keeping what is on screen until
    /// the answer lands. Workflows that gave no history get another chance.
    pub fn runs_refresh(&mut self, now: Instant) {
        self.runs.history.retain(|_, h| h.runs > 0);
        self.runs.generation += 1;
        self.runs.in_flight = false;
        self.runs.want_load = true;
        self.runs.comment_waits = 0;
        self.runs.due = Some(now);
    }

    /// The timer fired: decide whether to fetch, and what.
    pub fn runs_tick(&mut self, now: Instant) -> Option<RunsFetch> {
        self.runs.due = None;
        if !self.runs_showing() {
            return None;
        }
        // Behind a full-frame agent session the tab is not visible, but it is
        // still the tab to come back to, so keep the timer alive.
        if self.mode == Mode::Harness || self.runs.throttled() {
            self.runs.paused = self.runs.throttled();
            self.runs.due = Some(now + POLL);
            return None;
        }
        self.runs.paused = false;
        let issue_id = self.runs.target.clone()?;
        let work = match &self.runs.data {
            Some(Ok(data)) if !self.runs.want_load => RunsWork::Poll {
                prs: data.prs.clone(),
                hidden: data.hidden,
            },
            _ => {
                if self.detail.comments.is_none() && self.runs.comment_waits < MAX_COMMENT_WAITS {
                    self.runs.comment_waits += 1;
                    self.runs.due = Some(now + COMMENT_WAIT);
                    return None;
                }
                RunsWork::Load {
                    text_refs: self.collect_pr_links(),
                }
            }
        };
        self.runs.in_flight = true;
        Some(RunsFetch {
            issue_id,
            generation: self.runs.generation,
            work,
            known: self.runs.known(),
        })
    }

    /// A fetch landed. Dropped when it has been overtaken; otherwise the only
    /// place a poll is re-armed.
    pub fn apply_runs(
        &mut self,
        issue_id: &str,
        generation: u64,
        result: Result<RunsUpdate, String>,
        at: DateTime<Utc>,
        now: Instant,
    ) {
        if generation != self.runs.generation || self.runs.target.as_deref() != Some(issue_id) {
            return;
        }
        self.runs.in_flight = false;
        let keep_on = self.runs.selected_node();
        match result {
            Ok(update) => {
                self.runs.history.extend(update.history);
                let estimates = forecast(&update.prs, &self.runs.history, at);
                self.runs.data = Some(Ok(RunsData {
                    prs: update.prs,
                    hidden: update.hidden,
                    estimates,
                    as_of: at,
                }));
                self.runs.stale = None;
                self.runs.want_load = false;
            }
            Err(e) if matches!(self.runs.data, Some(Ok(_))) => self.runs.stale = Some(e),
            Err(e) => self.runs.data = Some(Err(e)),
        }
        self.runs.reselect(keep_on);
        let active = matches!(&self.runs.data, Some(Ok(d)) if d.has_active_run());
        self.runs.due = (active && self.runs_showing()).then(|| now + POLL);
    }
}

/// Forecast every active run that has any jobs to forecast from.
fn forecast(
    prs: &[PrRuns],
    history: &HashMap<HistoryKey, Arc<History>>,
    at: DateTime<Utc>,
) -> HashMap<i64, RunEstimate> {
    let none = History::default();
    let mut estimates = HashMap::new();
    for entry in prs {
        for run in entry.runs.iter().flatten().filter(|r| r.is_active()) {
            let h = history
                .get(&HistoryKey::of(&entry.pr.pr, run))
                .map_or(&none, Arc::as_ref);
            estimates.insert(run.id, estimate(run, h, at));
        }
    }
    estimates
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::super::run_tree::RunRowKind;
    use super::*;
    use crate::provider::types::{IssueState, RepoIssues};
    use crate::runs::estimate::build_history;
    use crate::runs::{ActionsJob, ActionsRun, ActionsStep, LinkedPr};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 17, 4, 0, 0).unwrap()
    }

    fn issue(id: &str, number: u64) -> Issue {
        Issue {
            id: id.into(),
            number,
            title: "t".into(),
            body: "see #7".into(),
            state: IssueState::Open,
            url: "u".into(),
            author: "a".into(),
            assignees: vec![],
            labels: vec![],
            comment_count: 0,
            created_at: now(),
            updated_at: now(),
            closed_at: None,
        }
    }

    /// Two issues, the first selected, detail pane open and comments settled.
    fn app() -> App {
        let mut app = App::new("o".into(), None, false, false, "{number}".into());
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![issue("I_1", 1), issue("I_2", 2)],
        }]);
        app.selected = 1;
        app.runs.supported = true;
        app.open_detail();
        app.detail.comments = Some(vec![]);
        app
    }

    fn linked(number: u64, open: bool) -> LinkedPr {
        LinkedPr {
            pr: PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number,
            },
            title: "pr".into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            head_sha: "sha".into(),
            open,
            merged_at: (!open).then(now),
        }
    }

    fn job(id: i64, status: &str, conclusion: Option<&str>) -> ActionsJob {
        ActionsJob {
            id,
            name: format!("job{id}"),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            started_at: Some(now()),
            completed_at: conclusion.map(|_| now()),
            html_url: Some(format!("https://github.com/job/{id}")),
            steps: vec![ActionsStep {
                name: "step".into(),
                status: status.into(),
                conclusion: conclusion.map(str::to_string),
                started_at: Some(now()),
                completed_at: conclusion.map(|_| now()),
            }],
        }
    }

    fn run(id: i64, status: &str, conclusion: Option<&str>, jobs: Vec<ActionsJob>) -> ActionsRun {
        ActionsRun {
            id,
            workflow_id: 1,
            name: Some("CI".into()),
            event: "push".into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            run_number: 1,
            run_started_at: Some(now()),
            updated_at: Some(now()),
            html_url: format!("https://github.com/run/{id}"),
            jobs,
        }
    }

    fn update(prs: Vec<PrRuns>) -> RunsUpdate {
        RunsUpdate {
            prs,
            hidden: 0,
            history: vec![],
        }
    }

    fn active_update() -> RunsUpdate {
        update(vec![PrRuns {
            pr: linked(5, true),
            runs: Ok(vec![run(
                10,
                "in_progress",
                None,
                vec![
                    job(100, "completed", Some("success")),
                    job(101, "in_progress", None),
                ],
            )]),
        }])
    }

    fn done_update() -> RunsUpdate {
        update(vec![PrRuns {
            pr: linked(5, true),
            runs: Ok(vec![run(
                10,
                "completed",
                Some("success"),
                vec![job(100, "completed", Some("success"))],
            )]),
        }])
    }

    /// Show the tab and run the debounced load, returning the fetch.
    fn loaded(app: &mut App, t: Instant) -> RunsFetch {
        app.toggle_runs_tab(t);
        app.runs_tick(t + DEBOUNCE).expect("a load")
    }

    #[test]
    fn showing_the_tab_arms_a_debounced_load() {
        let mut app = app();
        let t = Instant::now();
        app.toggle_runs_tab(t);
        assert_eq!(app.runs.due, Some(t + DEBOUNCE));
        let fetch = app.runs_tick(t + DEBOUNCE).unwrap();
        assert_eq!(fetch.issue_id, "I_1");
        let RunsWork::Load { text_refs } = fetch.work else {
            panic!("expected a load")
        };
        assert_eq!(text_refs.len(), 1, "body text refs ride along");
    }

    #[test]
    fn the_tab_is_not_available_when_the_provider_has_no_runs() {
        let mut app = app();
        app.runs.supported = false;
        app.toggle_runs_tab(Instant::now());
        assert!(!app.runs_showing());
        assert!(app.runs.due.is_none());
    }

    #[test]
    fn a_load_waits_for_the_thread_but_not_forever() {
        let mut app = app();
        app.detail.comments = None;
        let t = Instant::now();
        app.toggle_runs_tab(t);
        for _ in 0..MAX_COMMENT_WAITS {
            assert!(app.runs_tick(t).is_none());
            assert!(app.runs.due.is_some(), "still waiting");
        }
        assert!(app.runs_tick(t).is_some(), "loads without the thread");
    }

    #[test]
    fn an_active_run_arms_the_next_poll_and_a_finished_one_does_not() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        assert_eq!(app.runs.due, Some(t + POLL));
        let poll = app.runs_tick(t + POLL).unwrap();
        assert!(matches!(poll.work, RunsWork::Poll { .. }));
        app.apply_runs("I_1", poll.generation, Ok(done_update()), now(), t);
        assert!(app.runs.due.is_none(), "nothing left to watch");
    }

    #[test]
    fn a_merged_pr_never_keeps_polling_alive() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        let merged = update(vec![PrRuns {
            pr: linked(5, false),
            runs: Ok(vec![run(10, "in_progress", None, vec![])]),
        }]);
        app.apply_runs("I_1", fetch.generation, Ok(merged), now(), t);
        assert!(app.runs.due.is_none());
    }

    #[test]
    fn a_hidden_tab_does_not_poll() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        app.toggle_runs_tab(t);
        assert!(app.runs.due.is_none(), "flipping away disarms");
        assert!(app.runs_tick(t).is_none());
    }

    #[test]
    fn flipping_back_resumes_an_active_issue_without_a_reload() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        app.toggle_runs_tab(t);
        app.toggle_runs_tab(t);
        assert_eq!(app.runs.due, Some(t));
        assert!(matches!(
            app.runs_tick(t).unwrap().work,
            RunsWork::Poll { .. }
        ));
    }

    #[test]
    fn a_response_for_a_previous_selection_is_dropped() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.selected = 2;
        app.runs_follow_selection(t);
        assert_eq!(app.runs.target.as_deref(), Some("I_2"));
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        assert!(app.runs.data.is_none());
    }

    #[test]
    fn walking_the_list_only_loads_where_it_stops() {
        let mut app = app();
        let t = Instant::now();
        app.toggle_runs_tab(t);
        app.selected = 2;
        app.runs_follow_selection(t + DEBOUNCE / 2);
        assert_eq!(app.runs.due, Some(t + DEBOUNCE / 2 + DEBOUNCE));
        assert_eq!(app.runs.target.as_deref(), Some("I_2"));
    }

    #[test]
    fn refresh_overtakes_whatever_is_out() {
        let mut app = app();
        let t = Instant::now();
        let first = loaded(&mut app, t);
        app.apply_runs("I_1", first.generation, Ok(done_update()), now(), t);
        let late = first.generation;
        app.runs_refresh(t);
        let again = app.runs_tick(t).unwrap();
        assert!(matches!(again.work, RunsWork::Load { .. }));
        assert_ne!(again.generation, late);
    }

    #[test]
    fn refresh_retries_workflows_that_gave_no_history() {
        let mut app = app();
        let key = |id| HistoryKey {
            owner: "o".into(),
            repo: "r".into(),
            workflow_id: id,
            event: "push".into(),
        };
        app.runs.history.insert(key(1), Arc::default());
        let learned = build_history(&[run(1, "completed", Some("success"), vec![])]);
        app.runs.history.insert(key(2), Arc::new(learned));
        app.runs_refresh(Instant::now());
        assert!(!app.runs.history.contains_key(&key(1)));
        assert!(app.runs.history.contains_key(&key(2)));
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_answer() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        app.runs_refresh(t);
        let again = app.runs_tick(t).unwrap();
        app.apply_runs("I_1", again.generation, Err("boom".into()), now(), t);
        assert!(matches!(app.runs.data, Some(Ok(_))));
        assert_eq!(app.runs.stale.as_deref(), Some("boom"));
        assert_eq!(app.runs.due, Some(t + POLL), "still watching");
    }

    #[test]
    fn a_failed_first_load_shows_the_error_and_stops() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Err("boom".into()), now(), t);
        assert_eq!(app.runs.data, Some(Err("boom".into())));
        assert!(app.runs.due.is_none());
    }

    #[test]
    fn a_low_budget_holds_polling_until_it_resets() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        let budget = |reset| {
            Some(RateLimitData {
                remaining: RATE_FLOOR - 1,
                limit: 5000,
                reset,
                last_cost: None,
            })
        };
        app.runs.budget = budget(Utc::now().timestamp() + 600);
        assert!(app.runs_tick(t).is_none());
        assert!(app.runs.paused);
        assert!(app.runs.due.is_some(), "tries again later");
        app.runs.budget = budget(Utc::now().timestamp() - 1);
        assert!(app.runs_tick(t).is_some(), "a passed reset frees it");
        assert!(!app.runs.paused);
    }

    #[test]
    fn behind_a_harness_session_the_timer_idles_without_fetching() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        app.mode = Mode::Harness;
        assert!(app.runs_tick(t).is_none());
        assert_eq!(app.runs.due, Some(t + POLL));
    }

    #[test]
    fn finished_jobs_and_held_history_are_not_asked_for_again() {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        let mut landed = active_update();
        let key = HistoryKey {
            owner: "o".into(),
            repo: "r".into(),
            workflow_id: 1,
            event: "push".into(),
        };
        landed.history.push((key.clone(), Arc::default()));
        landed.prs[0].runs.as_mut().unwrap().push(run(
            11,
            "completed",
            Some("success"),
            vec![job(200, "completed", Some("success"))],
        ));
        app.apply_runs("I_1", fetch.generation, Ok(landed), now(), t);
        let known = app.runs_tick(t).unwrap().known;
        assert!(known.history.contains(&key));
        assert!(known.finished_jobs.contains_key(&11));
        assert!(!known.finished_jobs.contains_key(&10), "still running");
    }

    #[test]
    fn active_and_failed_start_open_and_success_collapses_to_one_line() {
        let data = RunsData {
            prs: vec![
                PrRuns {
                    pr: linked(5, true),
                    runs: Ok(vec![
                        run(
                            10,
                            "in_progress",
                            None,
                            vec![
                                job(100, "completed", Some("success")),
                                job(101, "in_progress", None),
                            ],
                        ),
                        run(
                            11,
                            "completed",
                            Some("success"),
                            vec![job(110, "completed", Some("success"))],
                        ),
                    ]),
                },
                PrRuns {
                    pr: linked(4, false),
                    runs: Ok(vec![run(
                        20,
                        "completed",
                        Some("success"),
                        vec![job(200, "completed", Some("success"))],
                    )]),
                },
            ],
            hidden: 3,
            ..RunsData::default()
        };
        let kinds: Vec<u8> = data.rows(&HashMap::new()).iter().map(|r| r.depth).collect();
        // PR5 > run10 > job100, job101 > step, run11; PR4 collapsed; +3 more.
        assert_eq!(kinds, vec![0, 1, 2, 2, 3, 1, 0, 0]);
    }

    #[test]
    fn a_failed_job_starts_open_with_its_steps() {
        let data = RunsData {
            prs: vec![PrRuns {
                pr: linked(5, true),
                runs: Ok(vec![run(
                    10,
                    "completed",
                    Some("failure"),
                    vec![job(100, "completed", Some("failure"))],
                )]),
            }],
            ..RunsData::default()
        };
        assert_eq!(data.rows(&HashMap::new()).len(), 4);
    }

    #[test]
    fn a_pr_that_cannot_be_read_shows_its_error_under_it() {
        let data = RunsData {
            prs: vec![PrRuns {
                pr: linked(5, true),
                runs: Err("no access".into()),
            }],
            ..RunsData::default()
        };
        let rows = data.rows(&HashMap::new());
        assert_eq!(rows[1].kind, RunRowKind::Failed { pr: 0 });
    }

    fn app_with_tree() -> App {
        let mut app = app();
        let t = Instant::now();
        let fetch = loaded(&mut app, t);
        app.apply_runs("I_1", fetch.generation, Ok(active_update()), now(), t);
        app.focus = Focus::Detail;
        app
    }

    #[test]
    fn folds_survive_a_poll_and_the_cursor_stays_on_its_node() {
        let mut app = app_with_tree();
        let t = Instant::now();
        app.runs_move(3); // job101
        app.runs_toggle();
        assert_eq!(app.runs.rows().len(), 4, "step hidden");
        let poll = app.runs_tick(t).unwrap();
        app.apply_runs("I_1", poll.generation, Ok(active_update()), now(), t);
        assert_eq!(app.runs.rows().len(), 4, "still folded");
        assert_eq!(app.runs.selected_node(), Some(RunNode::Job(101)));
    }

    #[test]
    fn left_folds_then_climbs_then_leaves_for_the_list() {
        let mut app = app_with_tree();
        app.runs_move(3); // job101, open
        app.runs_left();
        assert_eq!(app.runs.sel, 3, "folded in place");
        app.runs_left();
        assert_eq!(app.runs.selected_node(), Some(RunNode::Run(10)));
        app.runs_left(); // run folded
        app.runs_left(); // up to the PR
        app.runs_left(); // PR folded
        assert_eq!(app.runs.sel, 0);
        assert!(!app.runs.rows()[0].expanded);
        assert_eq!(app.focus, Focus::Detail);
        app.runs_left();
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn right_unfolds_then_steps_in() {
        let mut app = app_with_tree();
        app.runs_left(); // PR folded
        app.runs_right();
        assert!(app.runs.rows()[0].expanded);
        assert_eq!(app.runs.sel, 0);
        app.runs_right();
        assert_eq!(app.runs.sel, 1);
    }

    #[test]
    fn fold_all_and_unfold_all_keep_the_cursor_valid() {
        let mut app = app_with_tree();
        app.runs_jump_end();
        app.runs_set_all_folded(true);
        assert_eq!(app.runs.rows().len(), 1);
        assert_eq!(app.runs.sel, 0);
        app.runs_set_all_folded(false);
        assert_eq!(app.runs.rows().len(), 6, "PR, run, two jobs, two steps");
    }

    #[test]
    fn tab_walks_pr_and_run_headers_and_wraps() {
        let mut app = app_with_tree();
        app.runs_next_header(1);
        assert_eq!(app.runs.sel, 1);
        app.runs_next_header(1);
        assert_eq!(app.runs.sel, 0);
        app.runs_next_header(-1);
        assert_eq!(app.runs.sel, 1);
    }

    #[test]
    fn open_targets_the_row_and_a_step_opens_its_job() {
        let mut app = app_with_tree();
        assert_eq!(
            app.runs_selected_url().as_deref(),
            Some("https://github.com/o/r/pull/5")
        );
        app.runs_move(1);
        assert_eq!(
            app.runs_selected_url().as_deref(),
            Some("https://github.com/run/10")
        );
        app.runs_jump_end();
        assert_eq!(
            app.runs_selected_url().as_deref(),
            Some("https://github.com/job/101")
        );
        app.focus = Focus::List;
        assert_eq!(app.runs_selected_url(), None, "the list keeps `o`");
    }

    #[test]
    fn closing_the_pane_goes_back_to_comments_and_keeps_what_was_learned() {
        let mut app = app_with_tree();
        let key = HistoryKey {
            owner: "o".into(),
            repo: "r".into(),
            workflow_id: 1,
            event: "push".into(),
        };
        app.runs.history.insert(key.clone(), Arc::default());
        app.close_detail();
        assert_eq!(app.runs.tab, DetailTab::Comments);
        assert!(app.runs.data.is_none() && app.runs.due.is_none());
        assert!(app.runs.history.contains_key(&key));
    }

    #[test]
    fn switching_org_forgets_it_all_but_not_whether_runs_are_supported() {
        let mut app = app_with_tree();
        let before = app.runs.generation;
        app.runs.folds.insert(RunNode::Run(1), true);
        app.switch_org("other".into());
        assert!(app.runs.supported);
        assert!(app.runs.folds.is_empty());
        assert!(app.runs.generation > before, "generations only increase");
    }
}
