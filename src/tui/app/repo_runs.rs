//! The repo workflows view (#266, ADR 0014): what the detail pane shows while a
//! repo header is selected, and when it may ask for more.
//!
//! Scheduling works like the issue tab's (ADR 0007), as data: `due` says when
//! the event loop calls [`App::repo_runs_tick`], which decides what is sent.
//! Pages are only ever read one span at a time, and only a landed read sets
//! `poll_at`, so two reads of one repo's pages never overlap. A completed run's
//! jobs are read when it is unfolded, beside any page read, and kept on disk.
//!
//! The ETA history and the REST budget are the issue tab's: both are about the
//! same workflows and the same budget.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use super::prelude::*;
use super::repo_run_tree::RepoNode;
use super::runs::{DEBOUNCE, POLL};
use crate::cache::run_jobs::{RunJobsCache, RunKey};
use crate::ci::estimate::{History, RunEstimate, estimate};
use crate::ci::repo::{PagesRequest, PagesUpdate};
use crate::ci::{CiCapabilities, CiJob, CiRun, HistoryKey, RunFilter, Workflow};

/// How the view groups runs. One level only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    Flat,
    #[default]
    Status,
    Workflow,
    Branch,
}

impl Grouping {
    pub fn label(self) -> &'static str {
        match self {
            Grouping::Flat => "flat",
            Grouping::Status => "status",
            Grouping::Workflow => "workflow",
            Grouping::Branch => "branch",
        }
    }

    pub fn from_label(s: &str) -> Option<Grouping> {
        [
            Grouping::Flat,
            Grouping::Status,
            Grouping::Workflow,
            Grouping::Branch,
        ]
        .into_iter()
        .find(|g| g.label() == s)
    }

    /// `v`: the next one round.
    pub fn next(self) -> Grouping {
        match self {
            Grouping::Flat => Grouping::Status,
            Grouping::Status => Grouping::Workflow,
            Grouping::Workflow => Grouping::Branch,
            Grouping::Branch => Grouping::Flat,
        }
    }
}

/// A repo's run filter and text search. Per repo, session-only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFilter {
    pub run: RunFilter,
    pub text: String,
}

/// Values seen in a repo's runs this session, which the filter menu offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunVocab {
    pub events: Vec<String>,
    pub branches: Vec<String>,
    pub actors: Vec<String>,
}

impl RunVocab {
    fn absorb(&mut self, runs: &[CiRun]) {
        fn add(list: &mut Vec<String>, v: Option<&String>) {
            if let Some(v) = v
                && !list.contains(v)
            {
                list.push(v.clone());
            }
        }
        for run in runs {
            add(&mut self.events, Some(&run.event));
            add(&mut self.branches, run.branch.as_ref());
            add(&mut self.actors, run.actor.as_ref());
        }
        for list in [&mut self.events, &mut self.branches, &mut self.actors] {
            list.sort_by_key(|s| s.to_lowercase());
        }
    }
}

/// One fetch for the event loop to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoFetch {
    Pages {
        generation: u64,
        req: Box<PagesRequest>,
    },
    Jobs {
        generation: u64,
        owner: String,
        repo: String,
        run_id: i64,
        attempt: u32,
    },
    Workflows {
        owner: String,
        repo: String,
    },
}

/// What the view holds for its repo.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoRunsData {
    /// Every loaded page's runs, newest first, each once.
    pub runs: Vec<CiRun>,
    /// How many pages are loaded.
    pub pages: u32,
    /// Runs matching the run filter in all.
    pub total: usize,
    /// Forecasts for active runs, by run id, as of `as_of`.
    pub estimates: HashMap<i64, RunEstimate>,
    pub as_of: DateTime<Utc>,
}

impl RepoRunsData {
    pub fn has_active_run(&self) -> bool {
        self.runs.iter().any(CiRun::is_active)
    }

    /// Runs the backend has that no page loaded yet.
    pub fn more(&self) -> usize {
        self.total.saturating_sub(self.runs.len())
    }
}

/// What the next page read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Every loaded page again (at least the first).
    Reload,
    /// The next page after the loaded ones.
    More,
}

#[derive(Debug, Default)]
pub struct RepoRunsState {
    /// The repo the view is about, by name. Its owner is the org.
    pub target: Option<String>,
    pub owner: String,
    /// `None` until the first answer for `target`.
    pub data: Option<std::result::Result<RepoRunsData, String>>,
    /// When the event loop should call [`App::repo_runs_tick`].
    pub due: Option<Instant>,
    /// Only ever increases, so an overtaken response can always be recognised.
    /// No reset touches it.
    generation: u64,
    in_flight: bool,
    want: Option<Want>,
    /// Pages may be read from then: the debounce on entering the view.
    not_before: Option<Instant>,
    /// When the next poll is due. Only a landed page read sets it.
    poll_at: Option<Instant>,
    /// Completed runs whose jobs were asked for by an unfold, not yet sent.
    jobs_wanted: Vec<i64>,
    /// Jobs reads sent and not landed.
    pub jobs_loading: HashSet<i64>,
    /// Jobs reads that failed, by run id.
    pub jobs_failed: HashMap<i64, String>,
    /// A refresh failed; `data` is the last good answer.
    pub stale: Option<String>,
    /// Polling is held back by the REST budget.
    pub paused: bool,
    /// Row cursor into the view's rows.
    pub sel: usize,
    /// First visible row, remembered between frames.
    pub top: Cell<usize>,
    /// User fold choices; absent nodes take their default.
    pub folds: HashMap<RepoNode, bool>,
    pub group: Grouping,
    /// Run filter and text search, by repo name. Session-only.
    pub filters: HashMap<String, RepoFilter>,
    /// The workflows each repo defines, read once a session.
    pub workflows: HashMap<String, std::result::Result<Vec<Workflow>, String>>,
    workflows_asked: HashSet<String>,
    /// Events, branches and actors seen, by repo name.
    pub vocab: HashMap<String, RunVocab>,
    /// What the backend can push down; set at startup.
    pub capabilities: CiCapabilities,
    /// The completed-run cache (ADR 0014). Disabled until the event loop
    /// opens the org's.
    pub disk: RunJobsCache,
}

impl RepoRunsState {
    /// Point at another repo (or none). The filters, grouping, workflows and
    /// vocabulary stay; the runs and folds go, and anything still out is
    /// overtaken.
    fn retarget(&mut self, target: Option<String>, owner: &str, now: Instant) {
        self.target = target;
        self.owner = owner.to_string();
        self.drop_data();
        if self.target.is_some() {
            self.want = Some(Want::Reload);
            self.not_before = Some(now + DEBOUNCE);
        }
    }

    fn drop_data(&mut self) {
        self.data = None;
        self.stale = None;
        self.paused = false;
        self.sel = 0;
        self.top.set(0);
        self.folds.clear();
        self.in_flight = false;
        self.want = None;
        self.not_before = None;
        self.poll_at = None;
        self.jobs_wanted.clear();
        self.jobs_loading.clear();
        self.jobs_failed.clear();
        self.generation += 1;
        self.due = None;
    }

    /// The org changed: nothing learned about the old one stays.
    pub fn reset(&mut self) {
        self.target = None;
        self.drop_data();
        self.filters.clear();
        self.workflows.clear();
        self.workflows_asked.clear();
        self.vocab.clear();
    }

    /// The target repo's filter.
    pub fn filter(&self) -> RepoFilter {
        self.target
            .as_ref()
            .and_then(|r| self.filters.get(r))
            .cloned()
            .unwrap_or_default()
    }

    pub fn runs(&self) -> &[CiRun] {
        match &self.data {
            Some(Ok(d)) => &d.runs,
            _ => &[],
        }
    }

    /// When the event loop should next call the tick: the earliest of what is
    /// waiting.
    fn rearm(&mut self, now: Instant) {
        let pages = match (self.want, self.in_flight) {
            (Some(_), false) => Some(self.not_before.unwrap_or(now).max(now)),
            _ => None,
        };
        let jobs = (!self.jobs_wanted.is_empty()).then_some(now);
        let poll = if self.in_flight { None } else { self.poll_at };
        self.due = [pages, jobs, poll].into_iter().flatten().min();
    }
}

impl App {
    /// The repo whose header is selected, while the detail pane is open and the
    /// backend has CI: the view is showing.
    pub fn repo_view_repo(&self) -> Option<&str> {
        if !self.detail.open || !self.runs.supported {
            return None;
        }
        match self.rows.get(self.selected)? {
            Row::RepoHeader { repo_idx } => self.repos.get(*repo_idx).map(|r| r.repo.as_str()),
            _ => None,
        }
    }

    pub fn repo_view_showing(&self) -> bool {
        self.repo_view_repo().is_some()
    }

    pub fn selected_is_repo_header(&self) -> bool {
        matches!(self.rows.get(self.selected), Some(Row::RepoHeader { .. }))
    }

    /// Called once per event-loop pass: entering the view, or another repo's
    /// header, reads it afresh after the debounce; leaving drops it.
    pub fn repo_runs_follow(&mut self, now: Instant) {
        let current = self.repo_view_repo().map(str::to_string);
        if current != self.repo_runs.target {
            let org = self.org.clone();
            self.repo_runs.retarget(current, &org, now);
            self.repo_runs.rearm(now);
        }
    }

    /// `r` in the view: every loaded page again, the workflow list, and
    /// another chance for workflows that gave no history.
    pub fn repo_runs_refresh(&mut self, now: Instant) {
        let Some(repo) = self.repo_runs.target.clone() else {
            return;
        };
        self.runs.history.retain(|_, h| h.runs > 0);
        self.repo_runs.workflows_asked.remove(&repo);
        self.repo_runs.generation += 1;
        self.repo_runs.in_flight = false;
        self.repo_runs.want = Some(Want::Reload);
        self.repo_runs.not_before = None;
        self.repo_runs.rearm(now);
    }

    /// `L`: the next page. Does nothing when everything is loaded or a read is
    /// already out.
    pub fn repo_runs_load_more(&mut self, now: Instant) -> bool {
        let more = matches!(&self.repo_runs.data, Some(Ok(d)) if d.more() > 0);
        if !more || self.repo_runs.in_flight || self.repo_runs.want.is_some() {
            return false;
        }
        self.repo_runs.want = Some(Want::More);
        self.repo_runs.rearm(now);
        true
    }

    /// A new run filter for the target repo: the loaded pages no longer answer
    /// it, so read from page one.
    pub fn set_repo_run_filter(&mut self, run: RunFilter, now: Instant) {
        let Some(repo) = self.repo_runs.target.clone() else {
            return;
        };
        let entry = self.repo_runs.filters.entry(repo).or_default();
        if entry.run == run {
            return;
        }
        entry.run = run;
        let owner = self.repo_runs.owner.clone();
        let target = self.repo_runs.target.clone();
        self.repo_runs.retarget(target, &owner, now);
        self.repo_runs.not_before = None;
        self.repo_runs.rearm(now);
    }

    /// The text search: never sent, so it only changes what is shown.
    pub fn set_repo_run_text(&mut self, text: String) {
        let Some(repo) = self.repo_runs.target.clone() else {
            return;
        };
        let keep_on = self.repo_runs_selected_node();
        self.repo_runs.filters.entry(repo).or_default().text = text;
        self.repo_runs_reselect(keep_on);
    }

    /// `v`: the next grouping.
    pub fn cycle_repo_grouping(&mut self) {
        let keep_on = self.repo_runs_selected_node();
        self.repo_runs.group = self.repo_runs.group.next();
        self.repo_runs_reselect(keep_on);
    }

    /// Ask for a completed run's jobs because it is being unfolded: from the
    /// disk cache if it has them, otherwise from the backend.
    pub(super) fn want_jobs(&mut self, run_id: i64, now: Instant) {
        let s = &mut self.repo_runs;
        let Some(Ok(data)) = &mut s.data else { return };
        let Some(run) = data.runs.iter_mut().find(|r| r.id == run_id) else {
            return;
        };
        if run.is_active() || !run.jobs.is_empty() || s.jobs_loading.contains(&run_id) {
            return;
        }
        let Some(repo) = s.target.as_deref() else {
            return;
        };
        if let Some(jobs) = s.disk.get(&RunKey::of(&s.owner, repo, run)) {
            run.jobs = jobs.to_vec();
            return;
        }
        s.jobs_failed.remove(&run_id);
        s.jobs_loading.insert(run_id);
        if !s.jobs_wanted.contains(&run_id) {
            s.jobs_wanted.push(run_id);
        }
        s.rearm(now);
    }

    /// The timer fired: decide what to send.
    pub fn repo_runs_tick(&mut self, now: Instant) -> Vec<RepoFetch> {
        self.repo_runs.due = None;
        let Some(repo) = self.repo_runs.target.clone() else {
            return Vec::new();
        };
        if !self.repo_view_showing() {
            return Vec::new();
        }
        let throttled = self.runs.budget.is_some_and(|b| {
            b.remaining < super::runs::RATE_FLOOR && b.reset > Utc::now().timestamp()
        });
        // Behind a full-frame session the view is not visible, but it is still
        // the view to come back to, so keep the timer alive.
        if self.mode == Mode::Harness || throttled {
            self.repo_runs.paused = throttled;
            self.repo_runs.due = Some(now + POLL);
            return Vec::new();
        }
        self.repo_runs.paused = false;
        let s = &mut self.repo_runs;
        let owner = s.owner.clone();
        let mut out = Vec::new();

        if s.workflows_asked.insert(repo.clone()) {
            out.push(RepoFetch::Workflows {
                owner: owner.clone(),
                repo: repo.clone(),
            });
        }

        for run_id in std::mem::take(&mut s.jobs_wanted) {
            let attempt = s
                .runs()
                .iter()
                .find(|r| r.id == run_id)
                .map_or(1, |r| r.run_attempt);
            out.push(RepoFetch::Jobs {
                generation: s.generation,
                owner: owner.clone(),
                repo: repo.clone(),
                run_id,
                attempt,
            });
        }

        let ready = s.not_before.is_none_or(|t| t <= now);
        let poll_due = s.poll_at.is_some_and(|t| t <= now);
        if !s.in_flight && ((s.want.is_some() && ready) || (s.want.is_none() && poll_due)) {
            let loaded = match &s.data {
                Some(Ok(d)) => d.pages,
                _ => 0,
            };
            let (first, last) = match s.want {
                Some(Want::More) => (loaded + 1, loaded + 1),
                _ => (1, loaded.max(1)),
            };
            let jobs_for = s
                .runs()
                .iter()
                .filter(|r| r.is_active())
                .map(|r| r.id)
                .collect();
            out.push(RepoFetch::Pages {
                generation: s.generation,
                req: Box::new(PagesRequest {
                    owner,
                    repo,
                    filter: s.filter().run,
                    first,
                    last,
                    jobs_for,
                    known_history: self.runs.history.keys().cloned().collect(),
                }),
            });
            s.in_flight = true;
            s.not_before = None;
            s.poll_at = None;
        }
        s.rearm(now);
        out
    }

    /// A page read landed. Dropped when overtaken; otherwise the only place a
    /// poll is armed.
    pub fn apply_repo_pages(
        &mut self,
        repo: &str,
        generation: u64,
        (first, last): (u32, u32),
        result: std::result::Result<PagesUpdate, String>,
        at: DateTime<Utc>,
        now: Instant,
    ) {
        if generation != self.repo_runs.generation || self.repo_runs.target.as_deref() != Some(repo)
        {
            return;
        }
        let keep_on = self.repo_runs_selected_node();
        let s = &mut self.repo_runs;
        s.in_flight = false;
        let more = s.want == Some(Want::More);
        s.want = None;
        match result {
            Ok(update) => {
                self.runs.history.extend(update.history);
                let s = &mut self.repo_runs;
                let old: Vec<CiRun> = match s.data.take() {
                    Some(Ok(d)) => d.runs,
                    _ => Vec::new(),
                };
                let mut runs = update.runs;
                for run in runs.iter_mut().filter(|r| r.jobs.is_empty() && r.is_done()) {
                    let carried = old.iter().find(|o| {
                        o.id == run.id && o.run_attempt == run.run_attempt && o.is_done()
                    });
                    if let Some(o) = carried.filter(|o| !o.jobs.is_empty()) {
                        run.jobs = o.jobs.clone();
                    } else if let Some(jobs) = s.disk.get(&RunKey::of(&s.owner, repo, run)) {
                        run.jobs = jobs.to_vec();
                    }
                }
                s.disk.put_many(&s.owner, repo, runs.iter());
                s.vocab.entry(repo.to_string()).or_default().absorb(&runs);
                let runs = if more && first > 1 {
                    let mut all = old;
                    let ids: HashSet<i64> = all.iter().map(|r| r.id).collect();
                    all.extend(runs.into_iter().filter(|r| !ids.contains(&r.id)));
                    all
                } else {
                    runs
                };
                let estimates = forecast(&s.owner, repo, &runs, &self.runs.history, at);
                let s = &mut self.repo_runs;
                s.data = Some(Ok(RepoRunsData {
                    runs,
                    pages: last,
                    total: update.total,
                    estimates,
                    as_of: at,
                }));
                s.stale = None;
            }
            Err(e) if matches!(s.data, Some(Ok(_))) => s.stale = Some(e),
            Err(e) => s.data = Some(Err(e)),
        }
        self.repo_runs_reselect(keep_on);
        let active = matches!(&self.repo_runs.data, Some(Ok(d)) if d.has_active_run());
        let showing = self.repo_view_showing();
        let s = &mut self.repo_runs;
        s.poll_at = (active && showing).then(|| now + POLL);
        s.rearm(now);
    }

    /// A completed run's jobs landed.
    pub fn apply_repo_jobs(
        &mut self,
        repo: &str,
        generation: u64,
        run_id: i64,
        attempt: u32,
        result: std::result::Result<Vec<CiJob>, String>,
    ) {
        let s = &mut self.repo_runs;
        if generation != s.generation || s.target.as_deref() != Some(repo) {
            return;
        }
        s.jobs_loading.remove(&run_id);
        let keep_on = self.repo_runs_selected_node();
        let s = &mut self.repo_runs;
        match result {
            Ok(jobs) => {
                let Some(Ok(data)) = &mut s.data else { return };
                let Some(run) = data
                    .runs
                    .iter_mut()
                    .find(|r| r.id == run_id && r.run_attempt == attempt)
                else {
                    return;
                };
                run.jobs = jobs;
                let run = run.clone();
                s.disk.put_many(&s.owner, repo, [&run]);
            }
            Err(e) => {
                s.jobs_failed.insert(run_id, e);
            }
        }
        self.repo_runs_reselect(keep_on);
    }

    pub fn apply_repo_workflows(
        &mut self,
        repo: &str,
        result: std::result::Result<Vec<Workflow>, String>,
    ) {
        if result.is_err() {
            // Ask again on the next `r`.
            self.repo_runs.workflows_asked.remove(repo);
        }
        self.repo_runs.workflows.insert(repo.to_string(), result);
    }
}

/// Forecast every active run of the repo.
fn forecast(
    owner: &str,
    repo: &str,
    runs: &[CiRun],
    history: &HashMap<HistoryKey, Arc<History>>,
    at: DateTime<Utc>,
) -> HashMap<i64, RunEstimate> {
    let none = History::default();
    runs.iter()
        .filter(|r| r.is_active())
        .map(|run| {
            let h = history
                .get(&HistoryKey::new(owner, repo, run))
                .map_or(&none, Arc::as_ref);
            (run.id, estimate(run, h, at))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Instant;

    use chrono::TimeZone;

    use super::super::repo_run_tree::RepoRowKind;
    use super::*;
    use crate::ci::RunStatus;
    use crate::ci::repo::fake::run;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 8, 4, 0, 0).unwrap()
    }

    /// An app with the detail pane open on repo `r`'s header (row 0).
    fn app() -> App {
        let mut app = App::new("o".into(), None, false, false, "{number}".into());
        app.set_data(vec![
            RepoIssues {
                repo: "r".into(),
                repo_url: "u".into(),
                issues: vec![Issue::fixture("I_1", 1)],
            },
            RepoIssues {
                repo: "s".into(),
                repo_url: "u2".into(),
                issues: vec![Issue::fixture("I_2", 2)],
            },
        ]);
        app.runs.supported = true;
        app.selected = 0;
        app.open_detail();
        app
    }

    fn header(app: &App, idx: usize) -> usize {
        app.rows
            .iter()
            .position(|r| matches!(r, Row::RepoHeader { repo_idx } if *repo_idx == idx))
            .unwrap()
    }

    fn pages(fetches: &[RepoFetch]) -> Option<(u64, PagesRequest)> {
        fetches.iter().find_map(|f| match f {
            RepoFetch::Pages { generation, req } => Some((*generation, (**req).clone())),
            _ => None,
        })
    }

    fn update(runs: Vec<CiRun>, total: usize) -> PagesUpdate {
        PagesUpdate {
            runs,
            total,
            history: vec![],
        }
    }

    fn land(app: &mut App, g: u64, runs: Vec<CiRun>, total: usize, t: Instant) {
        app.apply_repo_pages("r", g, (1, 1), Ok(update(runs, total)), now(), t);
    }

    fn failed_job(id: i64) -> CiJob {
        CiJob {
            id,
            name: "build".into(),
            status: RunStatus::Failure,
            started_at: None,
            completed_at: None,
            html_url: None,
            steps: vec![],
        }
    }

    /// Enter the view and let the debounce pass: the first read is out.
    fn entered(app: &mut App, t: Instant) -> (u64, PagesRequest) {
        app.repo_runs_follow(t);
        pages(&app.repo_runs_tick(t + DEBOUNCE)).expect("a first read")
    }

    #[test]
    fn a_repo_header_with_the_pane_open_is_the_view() {
        let mut app = app();
        assert_eq!(app.repo_view_repo(), Some("r"));
        app.selected = 1;
        assert!(
            !app.repo_view_showing(),
            "an issue row keeps the issue detail"
        );
        app.selected = 0;
        app.close_detail();
        assert!(!app.repo_view_showing());
        let mut app = self::app();
        app.runs.supported = false;
        assert!(!app.repo_view_showing(), "no CI backend, no view");
    }

    #[test]
    fn entering_reads_page_one_after_the_debounce() {
        let mut app = app();
        let t = Instant::now();
        app.repo_runs_follow(t);
        assert_eq!(app.repo_runs.due, Some(t + DEBOUNCE));
        assert!(
            pages(&app.repo_runs_tick(t)).is_none(),
            "not before the debounce"
        );
        let (_, req) = pages(&app.repo_runs_tick(t + DEBOUNCE)).unwrap();
        assert_eq!((req.owner.as_str(), req.repo.as_str()), ("o", "r"));
        assert_eq!((req.first, req.last), (1, 1));
    }

    #[test]
    fn the_workflow_list_is_asked_for_once_a_session() {
        let mut app = app();
        let t = Instant::now();
        app.repo_runs_follow(t);
        let first = app.repo_runs_tick(t + DEBOUNCE);
        assert!(
            first
                .iter()
                .any(|f| matches!(f, RepoFetch::Workflows { .. }))
        );
        let again = app.repo_runs_tick(t + DEBOUNCE * 2);
        assert!(
            !again
                .iter()
                .any(|f| matches!(f, RepoFetch::Workflows { .. }))
        );
    }

    #[test]
    fn an_active_run_arms_a_poll_of_every_loaded_page() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Running)], 250, t);
        assert_eq!(app.repo_runs.due, Some(t + POLL));
        assert!(app.repo_runs_load_more(t));
        let (g, req) = pages(&app.repo_runs_tick(t)).unwrap();
        assert_eq!(
            (req.first, req.last),
            (2, 2),
            "load more reads only the next page"
        );
        let more = update(vec![run(4, RunStatus::Success)], 250);
        app.apply_repo_pages("r", g, (2, 2), Ok(more), now(), t);
        let data = app.repo_runs.data.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(data.pages, 2);
        assert_eq!(data.runs.len(), 2);
        let (_, req) = pages(&app.repo_runs_tick(t + POLL)).unwrap();
        assert_eq!(
            (req.first, req.last),
            (1, 2),
            "a poll re-reads every loaded page"
        );
        assert_eq!(
            req.jobs_for,
            HashSet::from([5]),
            "the active run's final jobs"
        );
    }

    #[test]
    fn load_more_does_nothing_when_everything_is_loaded() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Success)], 1, t);
        assert!(!app.repo_runs_load_more(t));
    }

    #[test]
    fn nothing_running_means_nothing_polls() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Success)], 1, t);
        assert_eq!(app.repo_runs.due, None);
    }

    #[test]
    fn reads_for_one_repo_never_overlap() {
        let mut app = app();
        let t = Instant::now();
        entered(&mut app, t);
        app.repo_runs_refresh(t);
        assert!(
            pages(&app.repo_runs_tick(t)).is_some(),
            "a refresh overtakes the read"
        );
        assert!(
            pages(&app.repo_runs_tick(t)).is_none(),
            "one read at a time"
        );
    }

    #[test]
    fn leaving_and_coming_back_reads_afresh_and_drops_what_was_out() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        app.selected = 1;
        app.repo_runs_follow(t);
        assert_eq!(app.repo_runs.target, None);
        app.selected = 0;
        app.repo_runs_follow(t);
        land(&mut app, g, vec![run(5, RunStatus::Success)], 1, t);
        assert!(app.repo_runs.data.is_none(), "the old read was overtaken");
    }

    #[test]
    fn another_header_is_another_repo() {
        let mut app = app();
        let t = Instant::now();
        entered(&mut app, t);
        app.selected = header(&app, 1);
        app.repo_runs_follow(t);
        assert_eq!(app.repo_runs.target.as_deref(), Some("s"));
    }

    #[test]
    fn behind_a_session_the_view_keeps_its_timer_but_sends_nothing() {
        let mut app = app();
        let t = Instant::now();
        app.repo_runs_follow(t);
        app.mode = Mode::Harness;
        assert!(app.repo_runs_tick(t + DEBOUNCE).is_empty());
        assert_eq!(app.repo_runs.due, Some(t + DEBOUNCE + POLL));
    }

    #[test]
    fn a_low_rest_budget_pauses_polling() {
        let mut app = app();
        let t = Instant::now();
        app.repo_runs_follow(t);
        app.runs.budget = Some(RateLimitData {
            remaining: 5,
            reset: Utc::now().timestamp() + 600,
            limit: 5000,
            last_cost: None,
        });
        assert!(app.repo_runs_tick(t + DEBOUNCE).is_empty());
        assert!(app.repo_runs.paused);
    }

    #[test]
    fn a_filter_change_reads_page_one_and_text_reads_nothing() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Success)], 1, t);
        app.set_repo_run_text("ci".into());
        assert_eq!(app.repo_runs.due, None, "text never leaves the client");
        app.set_repo_run_filter(
            RunFilter {
                branch: Some("main".into()),
                ..RunFilter::default()
            },
            t,
        );
        assert_eq!(app.repo_runs.due, Some(t), "no debounce on a filter change");
        let (_, req) = pages(&app.repo_runs_tick(t)).unwrap();
        assert_eq!(req.filter.branch.as_deref(), Some("main"));
        assert_eq!(app.repo_runs.filter().text, "ci", "the text search stays");
    }

    #[test]
    fn filters_are_per_repo() {
        let mut app = app();
        let t = Instant::now();
        entered(&mut app, t);
        app.set_repo_run_text("x".into());
        app.selected = header(&app, 1);
        app.repo_runs_follow(t);
        assert_eq!(app.repo_runs.filter().text, "");
        app.selected = 0;
        app.repo_runs_follow(t);
        assert_eq!(app.repo_runs.filter().text, "x");
    }

    #[test]
    fn unfolding_a_finished_run_reads_its_jobs_once_then_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::cache::run_jobs::path(dir.path());
        let mut app = app();
        app.repo_runs.disk = RunJobsCache::load(path.clone());
        app.repo_runs.group = Grouping::Flat;
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Failure)], 1, t);
        let rows = app.repo_runs.rows();
        assert!(
            rows[0].expandable && !rows[0].expanded,
            "no jobs yet: folded"
        );
        app.repo_runs_toggle(t);
        assert!(matches!(
            app.repo_runs.rows()[1].kind,
            RepoRowKind::LoadingJobs { .. }
        ));
        let fetches = app.repo_runs_tick(t);
        let jobs = fetches
            .iter()
            .filter(|f| matches!(f, RepoFetch::Jobs { run_id: 5, .. }))
            .count();
        assert_eq!(jobs, 1);
        app.repo_runs_toggle(t);
        app.repo_runs_toggle(t);
        assert!(
            !app.repo_runs_tick(t)
                .iter()
                .any(|f| matches!(f, RepoFetch::Jobs { .. })),
            "already loading: not asked twice"
        );
        app.apply_repo_jobs("r", g, 5, 1, Ok(vec![failed_job(50)]));
        assert!(matches!(
            app.repo_runs.rows()[1].kind,
            RepoRowKind::Job { .. }
        ));

        // Another session: the same run's jobs come from disk.
        let mut app = self::app();
        app.repo_runs.disk = RunJobsCache::load(path);
        app.repo_runs.group = Grouping::Flat;
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Failure)], 1, t);
        assert_eq!(
            app.repo_runs.runs()[0].jobs.len(),
            1,
            "read from disk on landing"
        );
        assert!(
            app.repo_runs.rows()[0].expanded,
            "a failed run with jobs opens"
        );
    }

    #[test]
    fn a_rerun_is_not_served_the_old_attempts_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.repo_runs.disk = RunJobsCache::load(crate::cache::run_jobs::path(dir.path()));
        let t = Instant::now();
        let mut first = run(5, RunStatus::Failure);
        first.jobs = vec![failed_job(50)];
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![first], 1, t);
        let mut rerun = run(5, RunStatus::Success);
        rerun.run_attempt = 2;
        app.repo_runs_refresh(t);
        let (g, _) = pages(&app.repo_runs_tick(t)).unwrap();
        land(&mut app, g, vec![rerun], 1, t);
        assert!(app.repo_runs.runs()[0].jobs.is_empty());
    }

    #[test]
    fn a_failed_jobs_read_says_so_under_the_run() {
        let mut app = app();
        app.repo_runs.group = Grouping::Flat;
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Success)], 1, t);
        app.repo_runs_toggle(t);
        app.apply_repo_jobs("r", g, 5, 1, Err("no access".into()));
        assert!(matches!(
            app.repo_runs.rows()[1].kind,
            RepoRowKind::JobsFailed { .. }
        ));
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_answer() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        land(&mut app, g, vec![run(5, RunStatus::Running)], 1, t);
        let (g, _) = pages(&app.repo_runs_tick(t + POLL)).unwrap();
        app.apply_repo_pages("r", g, (1, 1), Err("boom".into()), now(), t);
        assert_eq!(app.repo_runs.stale.as_deref(), Some("boom"));
        assert_eq!(app.repo_runs.runs().len(), 1);
    }

    #[test]
    fn the_vocabulary_learns_events_branches_and_actors() {
        let mut app = app();
        let t = Instant::now();
        let (g, _) = entered(&mut app, t);
        let mut a = run(5, RunStatus::Success);
        a.actor = Some("pgmac".into());
        let mut b = run(4, RunStatus::Success);
        b.branch = Some("dev".into());
        b.event = "pull_request".into();
        land(&mut app, g, vec![a, b], 2, t);
        let v = &app.repo_runs.vocab["r"];
        assert_eq!(v.branches, vec!["dev", "main"]);
        assert_eq!(v.events, vec!["pull_request", "push"]);
        assert_eq!(v.actors, vec!["pgmac"]);
    }

    #[test]
    fn grouping_cycles_round() {
        let mut g = Grouping::default();
        assert_eq!(g, Grouping::Status);
        let mut seen = vec![g];
        for _ in 0..3 {
            g = g.next();
            seen.push(g);
        }
        assert_eq!(g.next(), Grouping::Status);
        for g in seen {
            assert_eq!(Grouping::from_label(g.label()), Some(g));
        }
    }

    #[test]
    fn switching_org_forgets_the_repo_views_filters() {
        let mut app = app();
        let t = Instant::now();
        entered(&mut app, t);
        app.set_repo_run_text("x".into());
        app.switch_org("other".into());
        assert!(app.repo_runs.filters.is_empty());
        assert_eq!(app.repo_runs.target, None);
    }
}
