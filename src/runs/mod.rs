//! GitHub Actions workflow runs for the pull requests linked to an issue
//! (#222).
//!
//! The domain model, which PRs are worth showing, and the estimator. Nothing
//! here imports `tui`, and only [`fetch`] touches the provider: everything else
//! is pure, so the rules can be tested without a network or a terminal.

pub mod estimate;
pub mod fetch;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::provider::types::PrRef;

/// How many PRs the tab lists before the rest become a `+N more` line.
pub const MAX_PRS: usize = 5;

/// One step of a job, as the jobs API returns it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActionsStep {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// One job of a workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActionsJob {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub steps: Vec<ActionsStep>,
}

/// A workflow run plus its jobs. The list endpoints return each run once, at
/// its latest attempt, and the jobs endpoint defaults to that attempt too.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActionsRun {
    pub id: i64,
    pub workflow_id: i64,
    /// The workflow's name.
    #[serde(default)]
    pub name: Option<String>,
    pub event: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub run_number: u64,
    pub run_started_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub html_url: String,
    /// Filled in separately: the list endpoints do not carry jobs.
    #[serde(skip)]
    pub jobs: Vec<ActionsJob>,
}

impl ActionsRun {
    pub fn workflow(&self) -> &str {
        self.name.as_deref().unwrap_or("(unnamed workflow)")
    }

    pub fn is_active(&self) -> bool {
        self.status != "completed"
    }

    /// Whether this run finished without doing its work.
    pub fn is_failed(&self) -> bool {
        self.status == "completed" && is_failure(self.conclusion.as_deref())
    }
}

/// The conclusions that mean a run, job or step did not do its work. Skipped
/// and neutral are deliberately absent: both are a normal way to finish.
pub fn is_failure(conclusion: Option<&str>) -> bool {
    matches!(
        conclusion,
        Some("failure" | "timed_out" | "cancelled" | "startup_failure" | "action_required")
    )
}

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
    pub fn of(pr: &PrRef, run: &ActionsRun) -> Self {
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
pub fn latest_runs(runs: Vec<ActionsRun>) -> Vec<ActionsRun> {
    let mut latest: Vec<ActionsRun> = Vec::new();
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

/// A duration the way people read CI times: `45s`, `7m36s`, `1h02m`.
pub fn fmt_dur(d: TimeDelta) -> String {
    // Round to the nearest second rather than truncating.
    let secs = (d.num_milliseconds().max(0) + 500) / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

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

    fn run(workflow_id: i64, name: &str, event: &str, run_number: u64) -> ActionsRun {
        ActionsRun {
            id: run_number as i64,
            workflow_id,
            name: Some(name.into()),
            event: event.into(),
            status: "completed".into(),
            conclusion: Some("success".into()),
            run_number,
            run_started_at: None,
            updated_at: None,
            html_url: String::new(),
            jobs: vec![],
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

    #[test]
    fn fmt_dur_matches_gh_eta() {
        assert_eq!(fmt_dur(TimeDelta::seconds(45)), "45s");
        assert_eq!(fmt_dur(TimeDelta::seconds(456)), "7m36s");
        assert_eq!(fmt_dur(TimeDelta::minutes(62)), "1h02m");
        assert_eq!(fmt_dur(TimeDelta::milliseconds(59_600)), "1m00s");
        assert_eq!(fmt_dur(TimeDelta::seconds(-5)), "0s");
    }

    #[test]
    fn a_cancelled_run_is_a_failure_but_a_skipped_one_is_not() {
        assert!(is_failure(Some("cancelled")));
        assert!(!is_failure(Some("skipped")));
        assert!(!is_failure(Some("success")));
        assert!(!is_failure(None));
    }
}
