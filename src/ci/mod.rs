//! CI runs, in words no one backend owns (#266, ADR 0014).
//!
//! The issue tracker and the CI system are separate things that GitHub happens
//! to be both of. [`CiProvider`] is the CI half: listing a repo's runs and
//! workflows, a commit's runs, a run's jobs and the history an ETA is learned
//! from. Each backend maps its own words into [`RunStatus`] at the edge, so the
//! estimator, the trees and the renderer never read a backend's raw strings.
//!
//! Nothing here imports `tui`.

pub mod estimate;
pub mod repo;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::provider::error::Result;
use crate::provider::types::{PrRef, RateLimitData};

/// Where a run, job or step stands. One value replaces GitHub's
/// `status`/`conclusion` pair: everything not finished is one of the first
/// three, everything finished is one of the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[default]
    Queued,
    /// Waiting on an environment approval.
    Waiting,
    Running,
    Success,
    Failure,
    Cancelled,
    TimedOut,
    /// Finished, but needs someone to act before it counts.
    ActionRequired,
    /// Never got going: the workflow file or its runner was broken.
    StartupFailure,
    Skipped,
    Neutral,
    /// Superseded before it finished.
    Stale,
}

impl RunStatus {
    /// Every status, in the order the status grouping and the filter menu
    /// list them: what needs attention first.
    pub const ALL: [RunStatus; 12] = [
        RunStatus::Running,
        RunStatus::Waiting,
        RunStatus::Queued,
        RunStatus::Failure,
        RunStatus::TimedOut,
        RunStatus::StartupFailure,
        RunStatus::ActionRequired,
        RunStatus::Cancelled,
        RunStatus::Success,
        RunStatus::Skipped,
        RunStatus::Neutral,
        RunStatus::Stale,
    ];

    /// Not finished yet, so it can still change.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            RunStatus::Queued | RunStatus::Waiting | RunStatus::Running
        )
    }

    pub fn is_done(self) -> bool {
        !self.is_active()
    }

    /// Finished without doing its work. Skipped and neutral are deliberately
    /// absent: both are a normal way to finish.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            RunStatus::Failure
                | RunStatus::Cancelled
                | RunStatus::TimedOut
                | RunStatus::ActionRequired
                | RunStatus::StartupFailure
        )
    }

    /// The word shown and filtered on. Stable: it is also the serde name, so
    /// the completed-run cache reads back what it wrote.
    pub fn word(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Waiting => "waiting",
            RunStatus::Running => "running",
            RunStatus::Success => "success",
            RunStatus::Failure => "failure",
            RunStatus::Cancelled => "cancelled",
            RunStatus::TimedOut => "timed_out",
            RunStatus::ActionRequired => "action_required",
            RunStatus::StartupFailure => "startup_failure",
            RunStatus::Skipped => "skipped",
            RunStatus::Neutral => "neutral",
            RunStatus::Stale => "stale",
        }
    }

    pub fn from_word(word: &str) -> Option<RunStatus> {
        RunStatus::ALL.into_iter().find(|s| s.word() == word)
    }
}

/// One step of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiStep {
    pub name: String,
    pub status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// One job of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiJob {
    pub id: i64,
    pub name: String,
    pub status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub html_url: Option<String>,
    pub steps: Vec<CiStep>,
}

/// A workflow run, at its latest attempt, plus its jobs once read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiRun {
    pub id: i64,
    pub workflow_id: i64,
    /// The workflow's name.
    pub name: Option<String>,
    pub event: String,
    pub status: RunStatus,
    pub run_number: u64,
    /// Which execution of the run this is: a re-run is a new attempt of the
    /// same run. Part of the completed-run cache's key.
    pub run_attempt: u32,
    pub created_at: Option<DateTime<Utc>>,
    pub run_started_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub html_url: String,
    pub branch: Option<String>,
    /// What the run is about: a commit message's first line or a PR title.
    pub title: Option<String>,
    /// Who triggered it.
    pub actor: Option<String>,
    /// Filled in separately: listing runs does not carry jobs.
    pub jobs: Vec<CiJob>,
}

impl CiRun {
    pub fn workflow(&self) -> &str {
        self.name.as_deref().unwrap_or("(unnamed workflow)")
    }

    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }

    pub fn is_done(&self) -> bool {
        self.status.is_done()
    }

    pub fn is_failed(&self) -> bool {
        self.status.is_failure()
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
        Self::new(&pr.owner, &pr.repo, run)
    }

    pub fn new(owner: &str, repo: &str, run: &CiRun) -> Self {
        Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            workflow_id: run.workflow_id,
            event: run.event.clone(),
        }
    }
}

/// A workflow a repo defines.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
}

/// What a run list is narrowed to on the backend (#266). One value per field,
/// which is what GitHub can filter on. Text search is never here: it stays on
/// the client.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RunFilter {
    pub status: Option<RunStatus>,
    pub workflow: Option<Workflow>,
    pub event: Option<String>,
    pub branch: Option<String>,
    pub actor: Option<String>,
}

impl RunFilter {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        *self == RunFilter::default()
    }
}

/// One page of a repo's runs, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunPage {
    pub runs: Vec<CiRun>,
    /// How many runs match the filter in all, across every page.
    pub total: usize,
}

/// Which run filters a backend can send with a list call. The filter menu
/// offers only these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CiCapabilities {
    pub status: bool,
    pub workflow: bool,
    pub event: bool,
    pub branch: bool,
    pub actor: bool,
}

/// The CI half of a backend (ADR 0014).
#[async_trait]
pub trait CiProvider: Send + Sync {
    fn capabilities(&self) -> CiCapabilities;

    /// Page `page` (from 1) of a repo's runs matching `filter`, newest first,
    /// without jobs.
    async fn list_runs(
        &self,
        owner: &str,
        repo: &str,
        filter: &RunFilter,
        page: u32,
    ) -> Result<RunPage>;

    /// The workflows a repo defines.
    async fn list_workflows(&self, owner: &str, repo: &str) -> Result<Vec<Workflow>>;

    /// The runs of a commit, without their jobs.
    async fn head_runs(&self, owner: &str, repo: &str, sha: &str) -> Result<Vec<CiRun>>;

    /// The jobs of a run's latest attempt, with their steps.
    async fn run_jobs(&self, owner: &str, repo: &str, run_id: i64) -> Result<Vec<CiJob>>;

    /// Recent successful runs of a workflow for one trigger event, with jobs:
    /// what an ETA is learned from.
    async fn history_runs(
        &self,
        owner: &str,
        repo: &str,
        workflow_id: i64,
        event: &str,
    ) -> Result<Vec<CiRun>>;

    /// Most recently observed rate limit for these calls, when they draw on a
    /// different budget from the issue provider's.
    fn rate_limit(&self) -> Option<RateLimitData>;

    /// The backend's own web page of a repo's runs, narrowed to `filter`, for
    /// `o` on a group header. `None` when it has no such page.
    fn runs_page_url(&self, _owner: &str, _repo: &str, _filter: &RunFilter) -> Option<String> {
        None
    }
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
    use super::*;

    #[test]
    fn words_round_trip_for_every_status() {
        for s in RunStatus::ALL {
            assert_eq!(RunStatus::from_word(s.word()), Some(s));
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.word()), "serde name is the word");
        }
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
        assert!(RunStatus::Cancelled.is_failure());
        assert!(!RunStatus::Skipped.is_failure());
        assert!(!RunStatus::Success.is_failure());
    }

    #[test]
    fn active_and_failure_never_overlap() {
        for s in RunStatus::ALL {
            assert!(!(s.is_active() && s.is_failure()), "{s:?}");
        }
        assert!(!RunStatus::Skipped.is_failure() && !RunStatus::Neutral.is_failure());
    }
}
