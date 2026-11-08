//! GitHub Actions workflow runs for the pull requests linked to an issue
//! (#222).
//!
//! The domain model, which PRs are worth showing, and the newest-run rule. Nothing
//! here imports `tui`, and only [`fetch`] touches the provider: everything else
//! is pure, so the rules can be tested without a network or a terminal.

pub mod fetch;

use chrono::{DateTime, Utc};

use crate::ci::CiRun;
use crate::provider::types::PrRef;

/// How many PRs the tab lists before the rest become a `+N more` line.
pub const MAX_PRS: usize = 5;

/// A pull request linked to an issue, carrying what the tab needs to find its
/// runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedPr {
    pub pr: PrRef,
    pub title: String,
    pub url: String,
    pub head_sha: String,
    pub open: bool,
    pub merged_at: Option<DateTime<Utc>>,
}

impl LinkedPr {
    pub fn merged(&self) -> bool {
        self.merged_at.is_some()
    }
}

/// Which ETA history a run is compared with: same workflow, same trigger
/// event, because different events often run different jobs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistoryKey {
    pub owner: String,
    pub repo: String,
    pub workflow_id: i64,
    pub event: String,
}

impl HistoryKey {
    pub fn of(pr: &PrRef, run: &CiRun) -> Self {
        Self {
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            workflow_id: run.workflow_id,
            event: run.event.clone(),
        }
    }
}

/// Open and merged PRs only, open first, then merged newest first, capped at
/// [`MAX_PRS`]. Returns the kept PRs and how many open or merged PRs the cap
/// left out. Closed-unmerged PRs are dropped and not counted.
pub fn select_prs(prs: Vec<LinkedPr>) -> (Vec<LinkedPr>, usize) {
    let mut kept: Vec<LinkedPr> = prs.into_iter().filter(|p| p.open || p.merged()).collect();
    kept.sort_by(|a, b| {
        b.open
            .cmp(&a.open)
            .then_with(|| b.merged_at.cmp(&a.merged_at))
            .then_with(|| b.pr.number.cmp(&a.pr.number))
    });
    let hidden = kept.len().saturating_sub(MAX_PRS);
    kept.truncate(MAX_PRS);
    (kept, hidden)
}

/// Keep the newest run of each workflow and trigger event, sorted by workflow
/// name. A head SHA can collect several runs of one workflow, and only the
/// latest says where that commit stands. Push and pull-request runs of the same
/// workflow are different runs, so both stay.
pub fn latest_runs(runs: Vec<CiRun>) -> Vec<CiRun> {
    let mut latest: Vec<CiRun> = Vec::new();
    for run in runs {
        match latest
            .iter_mut()
            .find(|r| r.workflow_id == run.workflow_id && r.event == run.event)
        {
            Some(slot) if run.run_number > slot.run_number => *slot = run,
            Some(_) => {}
            None => latest.push(run),
        }
    }
    latest.sort_by(|a, b| {
        a.workflow()
            .cmp(b.workflow())
            .then_with(|| a.event.cmp(&b.event))
    });
    latest
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::ci::RunStatus;

    fn linked(number: u64, open: bool, merged_day: Option<u32>) -> LinkedPr {
        LinkedPr {
            pr: PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number,
            },
            title: format!("pr {number}"),
            url: String::new(),
            head_sha: "sha".into(),
            open,
            merged_at: merged_day.map(|d| Utc.with_ymd_and_hms(2026, 10, d, 0, 0, 0).unwrap()),
        }
    }

    fn run(workflow_id: i64, name: &str, event: &str, run_number: u64) -> CiRun {
        CiRun {
            id: run_number as i64,
            workflow_id,
            name: Some(name.into()),
            event: event.into(),
            status: RunStatus::Success,
            run_number,
            ..CiRun::default()
        }
    }

    #[test]
    fn select_prs_drops_closed_unmerged_and_puts_open_first() {
        let (kept, hidden) = select_prs(vec![
            linked(1, false, Some(1)),
            linked(2, false, None),
            linked(3, true, None),
            linked(4, false, Some(9)),
        ]);
        let order: Vec<u64> = kept.iter().map(|p| p.pr.number).collect();
        assert_eq!(order, vec![3, 4, 1], "open, then merged newest first");
        assert_eq!(hidden, 0, "a closed-unmerged PR is not 'more'");
    }

    #[test]
    fn select_prs_caps_and_counts_what_it_cut() {
        let prs = (1..=7).map(|n| linked(n, true, None)).collect();
        let (kept, hidden) = select_prs(prs);
        assert_eq!(kept.len(), MAX_PRS);
        assert_eq!(hidden, 2);
    }

    #[test]
    fn latest_runs_keeps_the_newest_per_workflow_and_event() {
        let kept = latest_runs(vec![
            run(1, "CI", "push", 4),
            run(1, "CI", "push", 7),
            run(1, "CI", "pull_request", 5),
            run(2, "Audit", "push", 2),
        ]);
        let got: Vec<(&str, &str, u64)> = kept
            .iter()
            .map(|r| (r.workflow(), r.event.as_str(), r.run_number))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Audit", "push", 2),
                ("CI", "pull_request", 5),
                ("CI", "push", 7)
            ]
        );
    }
}
