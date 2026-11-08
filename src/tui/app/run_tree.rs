//! The workflow-runs tab's tree (#222): the rows built from a landed answer,
//! and the cursor that walks them. See `runs.rs` for when answers land.

use super::prelude::*;
use crate::ci::estimate::RunEstimate;
use crate::ci::{CiRun, RunStatus};
use crate::runs::fetch::PrRuns;

/// A foldable node of the tree. Keyed by identity, not position, so folds
/// survive a poll.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RunNode {
    Pr(String),
    Run(i64),
    Job(i64),
}

/// What one row of the tree shows, as indices into [`RunsData::prs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunRowKind {
    Pr {
        pr: usize,
    },
    Run {
        pr: usize,
        run: usize,
    },
    Job {
        pr: usize,
        run: usize,
        job: usize,
    },
    Step {
        pr: usize,
        run: usize,
        job: usize,
        step: usize,
    },
    /// This PR's runs could not be read.
    Failed {
        pr: usize,
    },
    NoRuns,
    /// Open or merged PRs the cap left out.
    More(usize),
}

/// One row of the tree. Every row is exactly one screen row: the renderer
/// truncates rather than wraps, so `rows.len()` is the region's height and the
/// scroll cannot drift from what is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub kind: RunRowKind,
    pub depth: u8,
    pub node: Option<RunNode>,
    pub expandable: bool,
    pub expanded: bool,
}

/// The landed answer for one issue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunsData {
    pub prs: Vec<PrRuns>,
    pub hidden: usize,
    /// Forecasts for active runs, by run id, as of `as_of`.
    pub estimates: HashMap<i64, RunEstimate>,
    /// When this landed. Rendering is a pure function of `App`, so times on
    /// screen move with each poll rather than with the clock.
    pub as_of: DateTime<Utc>,
}

impl RunsData {
    /// Whether anything can still change, which is what keeps a poll going.
    /// Merged PRs do not count: nothing polls them.
    pub fn has_active_run(&self) -> bool {
        self.prs
            .iter()
            .filter(|p| p.pr.open)
            .filter_map(|p| p.runs.as_ref().ok())
            .flatten()
            .any(CiRun::is_active)
    }

    /// Build the tree for the current folds.
    pub fn rows(&self, folds: &HashMap<RunNode, bool>) -> Vec<RunRow> {
        let is_open = |node: &RunNode, default: bool| *folds.get(node).unwrap_or(&default);
        let mut rows = Vec::new();
        for (p, entry) in self.prs.iter().enumerate() {
            let node = RunNode::Pr(entry.pr.url.clone());
            let attention = entry.runs.as_ref().map_or(true, |runs| {
                runs.iter().any(|r| r.is_active() || r.is_failed())
            });
            let expanded = is_open(&node, attention);
            rows.push(RunRow {
                kind: RunRowKind::Pr { pr: p },
                depth: 0,
                node: Some(node),
                expandable: true,
                expanded,
            });
            if !expanded {
                continue;
            }
            let runs = match &entry.runs {
                Ok(runs) => runs,
                Err(_) => {
                    rows.push(leaf(RunRowKind::Failed { pr: p }, 1));
                    continue;
                }
            };
            if runs.is_empty() {
                rows.push(leaf(RunRowKind::NoRuns, 1));
            }
            for (r, run) in runs.iter().enumerate() {
                let node = RunNode::Run(run.id);
                let expandable = !run.jobs.is_empty();
                let expanded = expandable && is_open(&node, run.is_active() || run.is_failed());
                rows.push(RunRow {
                    kind: RunRowKind::Run { pr: p, run: r },
                    depth: 1,
                    node: Some(node),
                    expandable,
                    expanded,
                });
                if !expanded {
                    continue;
                }
                for (j, job) in run.jobs.iter().enumerate() {
                    let node = RunNode::Job(job.id);
                    let expandable = !job.steps.is_empty();
                    let attention = job.status == RunStatus::Running || job.status.is_failure();
                    let expanded = expandable && is_open(&node, attention);
                    rows.push(RunRow {
                        kind: RunRowKind::Job {
                            pr: p,
                            run: r,
                            job: j,
                        },
                        depth: 2,
                        node: Some(node),
                        expandable,
                        expanded,
                    });
                    if expanded {
                        rows.extend((0..job.steps.len()).map(|s| {
                            leaf(
                                RunRowKind::Step {
                                    pr: p,
                                    run: r,
                                    job: j,
                                    step: s,
                                },
                                3,
                            )
                        }));
                    }
                }
            }
        }
        if self.hidden > 0 {
            rows.push(leaf(RunRowKind::More(self.hidden), 0));
        }
        rows
    }
}

fn leaf(kind: RunRowKind, depth: u8) -> RunRow {
    RunRow {
        kind,
        depth,
        node: None,
        expandable: false,
        expanded: false,
    }
}

impl App {
    /// `↓`/`↑` and the page keys on the tab.
    pub fn runs_move(&mut self, delta: isize) {
        let last = self.runs.rows().len().saturating_sub(1);
        self.runs.sel = self.runs.sel.saturating_add_signed(delta).min(last);
    }

    pub fn runs_jump_top(&mut self) {
        self.runs.sel = 0;
    }

    pub fn runs_jump_end(&mut self) {
        self.runs.sel = self.runs.rows().len().saturating_sub(1);
    }

    /// `Tab`/`Shift+Tab` on the tab: PR and run headers, wrapping, the way it
    /// walks comment headers in the other tab.
    pub fn runs_next_header(&mut self, delta: isize) {
        let rows = self.runs.rows();
        let headers: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.depth <= 1 && r.node.is_some())
            .map(|(i, _)| i)
            .collect();
        if headers.is_empty() {
            return;
        }
        let n = headers.len() as isize;
        let at = headers.iter().rposition(|h| *h <= self.runs.sel);
        let next = match (at, delta > 0) {
            (Some(i), _) if headers[i] != self.runs.sel && delta < 0 => i as isize,
            (Some(i), _) => (i as isize + delta).rem_euclid(n),
            (None, true) => 0,
            (None, false) => n - 1,
        };
        self.runs.sel = headers[next as usize];
    }

    fn set_selected_fold(&mut self, open: bool) -> bool {
        let rows = self.runs.rows();
        let Some(row) = rows.get(self.runs.sel) else {
            return false;
        };
        match &row.node {
            Some(node) if row.expandable => {
                self.runs.folds.insert(node.clone(), open);
                true
            }
            _ => false,
        }
    }

    /// `Space`/`Enter` on the tab.
    pub fn runs_toggle(&mut self) {
        let open = self
            .runs
            .rows()
            .get(self.runs.sel)
            .is_some_and(|r| !r.expanded);
        self.set_selected_fold(open);
    }

    /// `→` on the tab: unfold, or step in to the first child.
    pub fn runs_right(&mut self) {
        let rows = self.runs.rows();
        let Some(row) = rows.get(self.runs.sel) else {
            return;
        };
        if row.expanded {
            self.runs_move(1);
        } else {
            self.set_selected_fold(true);
        }
    }

    /// `←` on the tab: fold, or step out to the parent, or out to the list from
    /// the top level.
    pub fn runs_left(&mut self) {
        let rows = self.runs.rows();
        let Some(row) = rows.get(self.runs.sel) else {
            self.focus = Focus::List;
            return;
        };
        if row.expanded {
            self.set_selected_fold(false);
        } else if row.depth == 0 {
            self.focus = Focus::List;
        } else if let Some(parent) = rows[..self.runs.sel]
            .iter()
            .rposition(|r| r.depth < row.depth)
        {
            self.runs.sel = parent;
        }
    }

    /// `[`/`]` on the tab: fold or unfold everything.
    pub fn runs_set_all_folded(&mut self, folded: bool) {
        let keep_on = self.runs.selected_node();
        let Some(Ok(data)) = &self.runs.data else {
            return;
        };
        let mut nodes = Vec::new();
        for entry in &data.prs {
            nodes.push(RunNode::Pr(entry.pr.url.clone()));
            for run in entry.runs.iter().flatten() {
                nodes.push(RunNode::Run(run.id));
                nodes.extend(run.jobs.iter().map(|j| RunNode::Job(j.id)));
            }
        }
        for node in nodes {
            self.runs.folds.insert(node, !folded);
        }
        self.runs.reselect(keep_on);
    }

    /// `o`: what the selected row links to, when the tab has the keys. A step
    /// has no page of its own, so it opens its job.
    pub fn runs_selected_url(&self) -> Option<String> {
        if !self.runs_showing() || self.focus != Focus::Detail {
            return None;
        }
        let Some(Ok(data)) = &self.runs.data else {
            return None;
        };
        let run_of = |pr: usize, run: usize| data.prs[pr].runs.as_ref().ok().map(|r| &r[run]);
        match self.runs.rows().get(self.runs.sel)?.kind {
            RunRowKind::Pr { pr } | RunRowKind::Failed { pr } => Some(data.prs[pr].pr.url.clone()),
            RunRowKind::Run { pr, run } => run_of(pr, run).map(|r| r.html_url.clone()),
            RunRowKind::Job { pr, run, job } | RunRowKind::Step { pr, run, job, .. } => {
                let run = run_of(pr, run)?;
                Some(
                    run.jobs[job]
                        .html_url
                        .clone()
                        .unwrap_or(run.html_url.clone()),
                )
            }
            RunRowKind::NoRuns | RunRowKind::More(_) => None,
        }
    }
}
