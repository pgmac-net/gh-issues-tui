//! The repo workflows view's tree (#266): runs grouped one level deep, the
//! rows built from them, and the cursor that walks them. See `repo_runs.rs` for
//! when runs land.
//!
//! The run filter is applied here too, not only on the backend: a backend
//! may push down a superset (GitHub has no `startup_failure` status filter), and
//! the text search never leaves the client.

use std::time::Instant;

use super::prelude::*;
use super::repo_runs::{Grouping, RepoFilter};
use crate::ci::{CiRun, RunFilter, RunStatus};

/// A foldable node of the tree, keyed by identity so folds survive a poll.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RepoNode {
    Group(String),
    Run(i64),
    Job(i64),
}

/// The status buckets of the status grouping, in the order they are shown:
/// what needs attention first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusBucket {
    Running,
    Waiting,
    Queued,
    Failed,
    Success,
    Other,
}

impl StatusBucket {
    pub fn of(s: RunStatus) -> StatusBucket {
        match s {
            RunStatus::Running => StatusBucket::Running,
            RunStatus::Waiting => StatusBucket::Waiting,
            RunStatus::Queued => StatusBucket::Queued,
            RunStatus::Success => StatusBucket::Success,
            s if s.is_failure() => StatusBucket::Failed,
            _ => StatusBucket::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusBucket::Running => "running",
            StatusBucket::Waiting => "waiting",
            StatusBucket::Queued => "queued",
            StatusBucket::Failed => "failed",
            StatusBucket::Success => "success",
            StatusBucket::Other => "other",
        }
    }

    /// A status that stands for the bucket, for its glyph.
    pub fn status(self) -> RunStatus {
        match self {
            StatusBucket::Running => RunStatus::Running,
            StatusBucket::Waiting => RunStatus::Waiting,
            StatusBucket::Queued => RunStatus::Queued,
            StatusBucket::Failed => RunStatus::Failure,
            StatusBucket::Success => RunStatus::Success,
            StatusBucket::Other => RunStatus::Neutral,
        }
    }
}

const ORDER: [StatusBucket; 6] = [
    StatusBucket::Running,
    StatusBucket::Waiting,
    StatusBucket::Queued,
    StatusBucket::Failed,
    StatusBucket::Success,
    StatusBucket::Other,
];

/// What a group is a group of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKey {
    Status(StatusBucket),
    Workflow(String),
    Branch(Option<String>),
}

impl GroupKey {
    pub fn label(&self) -> String {
        match self {
            GroupKey::Status(b) => b.label().to_string(),
            GroupKey::Workflow(w) => w.clone(),
            GroupKey::Branch(Some(b)) => b.clone(),
            GroupKey::Branch(None) => "(no branch)".into(),
        }
    }

    fn node(&self) -> RepoNode {
        RepoNode::Group(match self {
            GroupKey::Status(b) => format!("status:{}", b.label()),
            GroupKey::Workflow(w) => format!("workflow:{w}"),
            GroupKey::Branch(b) => format!("branch:{}", b.as_deref().unwrap_or("")),
        })
    }

    fn of(group: Grouping, run: &CiRun) -> Option<GroupKey> {
        match group {
            Grouping::Flat => None,
            Grouping::Status => Some(GroupKey::Status(StatusBucket::of(run.status))),
            Grouping::Workflow => Some(GroupKey::Workflow(run.workflow().to_string())),
            Grouping::Branch => Some(GroupKey::Branch(run.branch.clone())),
        }
    }
}

/// A group of runs, as indices into the loaded runs, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunGroup {
    pub key: GroupKey,
    pub runs: Vec<usize>,
}

/// What one row of the tree shows, as indices into the loaded runs and
/// [`RepoView::groups`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoRowKind {
    Group {
        group: usize,
    },
    Run {
        run: usize,
    },
    Job {
        run: usize,
        job: usize,
    },
    Step {
        run: usize,
        job: usize,
        step: usize,
    },
    LoadingJobs {
        run: usize,
    },
    JobsFailed {
        run: usize,
    },
    /// Runs the backend has that no page loaded yet.
    More(usize),
    /// Nothing loaded matches.
    Empty,
}

/// One row: one screen row, never wrapped, as in the issue tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    pub kind: RepoRowKind,
    pub depth: u8,
    pub node: Option<RepoNode>,
    pub expandable: bool,
    pub expanded: bool,
}

/// The groups and rows for the current data, filter, grouping and folds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoView {
    pub groups: Vec<RunGroup>,
    pub rows: Vec<RepoRow>,
}

/// Whether `run` passes the filter, run fields and text both.
pub fn run_matches(run: &CiRun, f: &RepoFilter) -> bool {
    let RunFilter {
        status,
        workflow,
        event,
        branch,
        actor,
    } = &f.run;
    if status.is_some_and(|s| s != run.status)
        || workflow.as_ref().is_some_and(|w| w.id != run.workflow_id)
        || event.as_ref().is_some_and(|e| *e != run.event)
        || branch
            .as_ref()
            .is_some_and(|b| run.branch.as_ref() != Some(b))
        || actor
            .as_ref()
            .is_some_and(|a| run.actor.as_ref() != Some(a))
    {
        return false;
    }
    let text = f.text.trim().to_lowercase();
    if text.is_empty() {
        return true;
    }
    let number = format!("#{}", run.run_number);
    [
        Some(run.workflow()),
        run.title.as_deref(),
        run.branch.as_deref(),
        run.actor.as_deref(),
        Some(number.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(&text))
}

fn leaf(kind: RepoRowKind, depth: u8) -> RepoRow {
    RepoRow {
        kind,
        depth,
        node: None,
        expandable: false,
        expanded: false,
    }
}

/// Group the visible runs. Status groups keep their fixed order; workflow and
/// branch groups are ordered by their newest run, which is the order they
/// first appear in, since runs come newest first.
pub fn group_runs(runs: &[CiRun], visible: &[usize], group: Grouping) -> Vec<RunGroup> {
    let mut groups: Vec<RunGroup> = Vec::new();
    for &i in visible {
        let Some(key) = GroupKey::of(group, &runs[i]) else {
            continue;
        };
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => g.runs.push(i),
            None => groups.push(RunGroup { key, runs: vec![i] }),
        }
    }
    if group == Grouping::Status {
        groups.sort_by_key(|g| match g.key {
            GroupKey::Status(b) => ORDER.iter().position(|o| *o == b),
            _ => None,
        });
    }
    groups
}

impl super::repo_runs::RepoRunsState {
    /// Build the tree. Pure over the state, like the issue tab's rows.
    pub fn view(&self) -> RepoView {
        let Some(Ok(data)) = &self.data else {
            return RepoView::default();
        };
        let filter = self.filter();
        let runs = &data.runs;
        let visible: Vec<usize> = (0..runs.len())
            .filter(|&i| run_matches(&runs[i], &filter))
            .collect();
        let is_open = |node: &RepoNode, default: bool| *self.folds.get(node).unwrap_or(&default);
        let mut rows = Vec::new();

        let push_run = |rows: &mut Vec<RepoRow>, r: usize, depth: u8| {
            let run = &runs[r];
            let node = RepoNode::Run(run.id);
            // A finished run with no jobs read yet can still unfold: that is
            // what reads them.
            let expandable = run.is_done() || !run.jobs.is_empty();
            let attention = (run.is_active() || run.is_failed()) && !run.jobs.is_empty();
            let expanded = expandable && is_open(&node, attention);
            rows.push(RepoRow {
                kind: RepoRowKind::Run { run: r },
                depth,
                node: Some(node),
                expandable,
                expanded,
            });
            if !expanded {
                return;
            }
            if run.jobs.is_empty() {
                let kind = if self.jobs_failed.contains_key(&run.id) {
                    RepoRowKind::JobsFailed { run: r }
                } else {
                    RepoRowKind::LoadingJobs { run: r }
                };
                rows.push(leaf(kind, depth + 1));
                return;
            }
            for (j, job) in run.jobs.iter().enumerate() {
                let node = RepoNode::Job(job.id);
                let expandable = !job.steps.is_empty();
                let attention = job.status == RunStatus::Running || job.status.is_failure();
                let expanded = expandable && is_open(&node, attention);
                rows.push(RepoRow {
                    kind: RepoRowKind::Job { run: r, job: j },
                    depth: depth + 1,
                    node: Some(node),
                    expandable,
                    expanded,
                });
                if expanded {
                    rows.extend((0..job.steps.len()).map(|s| {
                        leaf(
                            RepoRowKind::Step {
                                run: r,
                                job: j,
                                step: s,
                            },
                            depth + 2,
                        )
                    }));
                }
            }
        };

        let groups = group_runs(runs, &visible, self.group);
        if self.group == Grouping::Flat {
            for &r in &visible {
                push_run(&mut rows, r, 0);
            }
        } else {
            for (g, group) in groups.iter().enumerate() {
                let node = group.key.node();
                let attention = group
                    .runs
                    .iter()
                    .any(|&r| runs[r].is_active() || runs[r].is_failed());
                let expanded = is_open(&node, attention);
                rows.push(RepoRow {
                    kind: RepoRowKind::Group { group: g },
                    depth: 0,
                    node: Some(node),
                    expandable: true,
                    expanded,
                });
                if expanded {
                    for &r in &group.runs {
                        push_run(&mut rows, r, 1);
                    }
                }
            }
        }
        if visible.is_empty() {
            rows.push(leaf(RepoRowKind::Empty, 0));
        }
        if data.more() > 0 {
            rows.push(leaf(RepoRowKind::More(data.more()), 0));
        }
        RepoView { groups, rows }
    }

    pub fn rows(&self) -> Vec<RepoRow> {
        self.view().rows
    }

    /// The run under the cursor, or the run of the job or step under it.
    pub fn selected_run(&self) -> Option<&CiRun> {
        let Some(Ok(data)) = &self.data else {
            return None;
        };
        let run = match self.rows().get(self.sel)?.kind {
            RepoRowKind::Run { run }
            | RepoRowKind::Job { run, .. }
            | RepoRowKind::Step { run, .. }
            | RepoRowKind::LoadingJobs { run }
            | RepoRowKind::JobsFailed { run } => run,
            _ => return None,
        };
        data.runs.get(run)
    }
}

impl App {
    pub(super) fn repo_runs_selected_node(&self) -> Option<RepoNode> {
        self.repo_runs
            .rows()
            .get(self.repo_runs.sel)
            .and_then(|r| r.node.clone())
    }

    /// Put the cursor back on `node` after the rows changed, or clamp it.
    pub(super) fn repo_runs_reselect(&mut self, node: Option<RepoNode>) {
        let rows = self.repo_runs.rows();
        let found = node.and_then(|n| rows.iter().position(|r| r.node.as_ref() == Some(&n)));
        let s = &mut self.repo_runs;
        s.sel = found.unwrap_or(s.sel).min(rows.len().saturating_sub(1));
    }

    pub fn repo_runs_move(&mut self, delta: isize) {
        let last = self.repo_runs.rows().len().saturating_sub(1);
        self.repo_runs.sel = self.repo_runs.sel.saturating_add_signed(delta).min(last);
    }

    pub fn repo_runs_jump_top(&mut self) {
        self.repo_runs.sel = 0;
    }

    pub fn repo_runs_jump_end(&mut self) {
        self.repo_runs.sel = self.repo_runs.rows().len().saturating_sub(1);
    }

    /// `Tab`/`Shift+Tab`: group headers, or runs when flat, wrapping.
    pub fn repo_runs_next_header(&mut self, delta: isize) {
        let rows = self.repo_runs.rows();
        let headers: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.depth == 0 && r.node.is_some())
            .map(|(i, _)| i)
            .collect();
        if headers.is_empty() {
            return;
        }
        let sel = self.repo_runs.sel;
        let n = headers.len() as isize;
        let at = headers.iter().rposition(|h| *h <= sel);
        let next = match (at, delta > 0) {
            (Some(i), _) if headers[i] != sel && delta < 0 => i as isize,
            (Some(i), _) => (i as isize + delta).rem_euclid(n),
            (None, true) => 0,
            (None, false) => n - 1,
        };
        self.repo_runs.sel = headers[next as usize];
    }

    fn repo_runs_set_fold(&mut self, open: bool, now: Instant) -> bool {
        let rows = self.repo_runs.rows();
        let Some(row) = rows.get(self.repo_runs.sel) else {
            return false;
        };
        let Some(node) = row.node.clone().filter(|_| row.expandable) else {
            return false;
        };
        if open && let RepoNode::Run(id) = node {
            self.want_jobs(id, now);
        }
        self.repo_runs.folds.insert(node, open);
        true
    }

    /// `Space`/`Enter`.
    pub fn repo_runs_toggle(&mut self, now: Instant) {
        let open = self
            .repo_runs
            .rows()
            .get(self.repo_runs.sel)
            .is_some_and(|r| !r.expanded);
        self.repo_runs_set_fold(open, now);
    }

    /// `→`: unfold, or step in to the first child.
    pub fn repo_runs_right(&mut self, now: Instant) {
        let rows = self.repo_runs.rows();
        let Some(row) = rows.get(self.repo_runs.sel) else {
            return;
        };
        if row.expanded {
            self.repo_runs_move(1);
        } else {
            self.repo_runs_set_fold(true, now);
        }
    }

    /// `←`: fold, or step out to the parent, or out to the list from the top.
    pub fn repo_runs_left(&mut self, now: Instant) {
        let rows = self.repo_runs.rows();
        let sel = self.repo_runs.sel;
        let Some(row) = rows.get(sel) else {
            self.focus = Focus::List;
            return;
        };
        if row.expanded {
            self.repo_runs_set_fold(false, now);
        } else if row.depth == 0 {
            self.focus = Focus::List;
        } else if let Some(parent) = rows[..sel].iter().rposition(|r| r.depth < row.depth) {
            self.repo_runs.sel = parent;
        }
    }

    /// `[`/`]`: fold or unfold groups, runs and jobs that are loaded. Unfolding
    /// everything does not read every finished run's jobs: that would be a
    /// call per run.
    pub fn repo_runs_set_all_folded(&mut self, folded: bool) {
        let keep_on = self.repo_runs_selected_node();
        let view = self.repo_runs.view();
        let runs = self.repo_runs.runs().to_vec();
        let mut nodes: Vec<RepoNode> = view.groups.iter().map(|g| g.key.node()).collect();
        for run in &runs {
            if folded || !run.jobs.is_empty() {
                nodes.push(RepoNode::Run(run.id));
            }
            nodes.extend(run.jobs.iter().map(|j| RepoNode::Job(j.id)));
        }
        for node in nodes {
            self.repo_runs.folds.insert(node, !folded);
        }
        self.repo_runs_reselect(keep_on);
    }

    /// `o`: what the selected row links to. A step opens its job; a group or
    /// the more row has no page of its own here (the event layer asks the CI
    /// backend for one).
    pub fn repo_runs_selected_url(&self) -> Option<String> {
        if !self.repo_view_showing() || self.focus != Focus::Detail {
            return None;
        }
        let runs = self.repo_runs.runs();
        match self.repo_runs.rows().get(self.repo_runs.sel)?.kind {
            RepoRowKind::Run { run }
            | RepoRowKind::LoadingJobs { run }
            | RepoRowKind::JobsFailed { run } => runs.get(run).map(|r| r.html_url.clone()),
            RepoRowKind::Job { run, job } | RepoRowKind::Step { run, job, .. } => {
                let run = runs.get(run)?;
                Some(
                    run.jobs[job]
                        .html_url
                        .clone()
                        .unwrap_or(run.html_url.clone()),
                )
            }
            _ => None,
        }
    }

    /// The run filter a group header stands for, on top of the view's own:
    /// what `o` on a header asks the backend's runs page to show.
    pub fn repo_runs_selected_group_filter(&self) -> Option<RunFilter> {
        let view = self.repo_runs.view();
        let RepoRowKind::Group { group } = view.rows.get(self.repo_runs.sel)?.kind else {
            return None;
        };
        let mut f = self.repo_runs.filter().run;
        match &view.groups[group].key {
            GroupKey::Status(b) => f.status = Some(b.status()),
            GroupKey::Workflow(name) => {
                let wf = self
                    .repo_runs
                    .runs()
                    .iter()
                    .find(|r| r.workflow() == name)
                    .map(|r| crate::ci::Workflow {
                        id: r.workflow_id,
                        name: name.clone(),
                    });
                f.workflow = wf;
            }
            GroupKey::Branch(b) => f.branch = b.clone(),
        }
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::super::repo_runs::{RepoRunsData, RepoRunsState};
    use super::*;
    use crate::ci::Workflow;
    use crate::ci::repo::fake::run;

    fn state(runs: Vec<CiRun>, total: usize, group: Grouping) -> RepoRunsState {
        let mut s = RepoRunsState::default();
        s.target = Some("r".into());
        s.data = Some(Ok(RepoRunsData {
            runs,
            pages: 1,
            total,
            ..RepoRunsData::default()
        }));
        s.group = group;
        s
    }

    fn named(id: i64, status: RunStatus, workflow: &str, branch: &str) -> CiRun {
        CiRun {
            name: Some(workflow.into()),
            workflow_id: if workflow == "CI" { 1 } else { 2 },
            branch: Some(branch.into()),
            ..run(id, status)
        }
    }

    fn group_labels(s: &RepoRunsState) -> Vec<String> {
        let view = s.view();
        view.rows
            .iter()
            .filter_map(|r| match r.kind {
                RepoRowKind::Group { group } => Some(view.groups[group].key.label()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn status_groups_put_what_needs_attention_first() {
        let s = state(
            vec![
                run(6, RunStatus::Success),
                run(5, RunStatus::Skipped),
                run(4, RunStatus::TimedOut),
                run(3, RunStatus::Queued),
                run(2, RunStatus::Running),
            ],
            5,
            Grouping::Status,
        );
        assert_eq!(
            group_labels(&s),
            vec!["running", "queued", "failed", "success", "other"]
        );
    }

    #[test]
    fn attention_groups_start_open_and_the_rest_folded() {
        let s = state(
            vec![run(2, RunStatus::Success), run(1, RunStatus::Running)],
            2,
            Grouping::Status,
        );
        let rows = s.rows();
        assert!(rows[0].expanded, "running opens");
        assert!(matches!(rows[1].kind, RepoRowKind::Run { .. }));
        assert!(!rows[2].expanded, "success stays folded");
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn workflow_and_branch_groups_follow_their_newest_run() {
        let runs = vec![
            named(3, RunStatus::Success, "Release", "v1"),
            named(2, RunStatus::Success, "CI", "main"),
            named(1, RunStatus::Success, "Release", "main"),
        ];
        let s = state(runs.clone(), 3, Grouping::Workflow);
        assert_eq!(group_labels(&s), vec!["Release", "CI"]);
        let s = state(runs, 3, Grouping::Branch);
        assert_eq!(group_labels(&s), vec!["v1", "main"]);
    }

    #[test]
    fn flat_has_no_group_rows() {
        let s = state(
            vec![run(2, RunStatus::Success), run(1, RunStatus::Success)],
            2,
            Grouping::Flat,
        );
        let rows = s.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.depth == 0));
    }

    #[test]
    fn the_filter_is_applied_on_the_client_too() {
        let mut s = state(
            vec![
                run(2, RunStatus::StartupFailure),
                run(1, RunStatus::Failure),
            ],
            2,
            Grouping::Flat,
        );
        s.filters.insert(
            "r".into(),
            RepoFilter {
                run: RunFilter {
                    status: Some(RunStatus::StartupFailure),
                    ..RunFilter::default()
                },
                text: String::new(),
            },
        );
        let rows = s.rows();
        assert_eq!(rows.len(), 1, "GitHub pushes down every failure; we narrow");
        assert!(matches!(rows[0].kind, RepoRowKind::Run { run: 0 }));
    }

    #[test]
    fn text_matches_workflow_title_branch_actor_and_number() {
        let mut r = named(42, RunStatus::Success, "Release", "main");
        r.title = Some("fix: flaky test".into());
        r.actor = Some("pgmac".into());
        let text = |t: &str| RepoFilter {
            text: t.into(),
            ..RepoFilter::default()
        };
        for hit in ["release", "FLAKY", "mai", "pgm", "#42"] {
            assert!(run_matches(&r, &text(hit)), "{hit}");
        }
        assert!(!run_matches(&r, &text("deploy")));
    }

    #[test]
    fn a_workflow_filter_matches_by_id() {
        let r = named(1, RunStatus::Success, "CI", "main");
        let f = |id| RepoFilter {
            run: RunFilter {
                workflow: Some(Workflow {
                    id,
                    name: "renamed".into(),
                }),
                ..RunFilter::default()
            },
            ..RepoFilter::default()
        };
        assert!(run_matches(&r, &f(1)));
        assert!(!run_matches(&r, &f(2)));
    }

    #[test]
    fn more_on_the_backend_is_the_last_row() {
        let s = state(vec![run(1, RunStatus::Success)], 431, Grouping::Flat);
        let rows = s.rows();
        assert!(matches!(rows.last().unwrap().kind, RepoRowKind::More(430)));
    }

    #[test]
    fn nothing_matching_says_so() {
        let mut s = state(vec![run(1, RunStatus::Success)], 1, Grouping::Status);
        s.filters.insert(
            "r".into(),
            RepoFilter {
                text: "nope".into(),
                ..RepoFilter::default()
            },
        );
        assert!(matches!(s.rows()[0].kind, RepoRowKind::Empty));
    }

    #[test]
    fn a_job_unfolds_to_its_steps() {
        let mut r = run(1, RunStatus::Running);
        r.jobs = vec![crate::ci::CiJob {
            id: 10,
            name: "build".into(),
            status: RunStatus::Running,
            started_at: None,
            completed_at: None,
            html_url: None,
            steps: vec![crate::ci::CiStep {
                name: "checkout".into(),
                status: RunStatus::Success,
                started_at: None,
                completed_at: None,
            }],
        }];
        let s = state(vec![r], 1, Grouping::Flat);
        let kinds: Vec<_> = s.rows().iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RepoRowKind::Run { run: 0 },
                RepoRowKind::Job { run: 0, job: 0 },
                RepoRowKind::Step {
                    run: 0,
                    job: 0,
                    step: 0
                },
            ]
        );
    }
}
