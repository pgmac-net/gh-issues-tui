//! Turning "the runs for this issue" into provider calls.
//!
//! [`load`] finds the issue's PRs and their runs from scratch; [`poll`] walks
//! the PRs already known, re-reading only what can still change. Both finish
//! with [`complete`], which adds jobs and any ETA history not yet held. Failures
//! are per PR wherever they can be, so one PR the token cannot read does not
//! take the others down.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt;
use futures::stream;

use crate::provider::IssueProvider;
use crate::provider::error::Result;
use crate::provider::types::PrRef;

use super::estimate::{History, build_history};
use super::{ActionsJob, ActionsRun, HistoryKey, LinkedPr, latest_runs, select_prs};

/// Concurrent provider calls per stage of a fetch, gh-eta's `fetchWorkers`.
const WORKERS: usize = 8;

/// One PR and its runs, or why they could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRuns {
    pub pr: LinkedPr,
    pub runs: std::result::Result<Vec<ActionsRun>, String>,
}

/// What a finished fetch hands back. `history` holds only what was new.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunsUpdate {
    pub prs: Vec<PrRuns>,
    /// Open or merged PRs the cap left out.
    pub hidden: usize,
    pub history: Vec<(HistoryKey, Arc<History>)>,
}

/// What a fetch may skip: jobs of runs already finished, and history already
/// held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub finished_jobs: HashMap<i64, Vec<ActionsJob>>,
    pub history: HashSet<HistoryKey>,
}

/// Find the issue's PRs and everything about their runs.
pub async fn load(
    client: &dyn IssueProvider,
    issue_id: &str,
    text_refs: &[PrRef],
    known: &Known,
) -> Result<RunsUpdate> {
    let linked = client.linked_prs(issue_id, text_refs).await?;
    let (prs, hidden) = select_prs(linked);
    let prs = read_runs(client, prs).await;
    Ok(complete(client, prs, hidden, known).await)
}

/// Re-read the runs of open PRs. Merged PRs keep what they have: nothing polls
/// them.
pub async fn poll(
    client: &dyn IssueProvider,
    previous: Vec<PrRuns>,
    hidden: usize,
    known: &Known,
) -> RunsUpdate {
    let prs = stream::iter(previous)
        .map(|entry| async move {
            if !entry.pr.open {
                return entry;
            }
            match head_runs(client, &entry.pr).await {
                Ok(runs) => PrRuns {
                    pr: entry.pr,
                    runs: Ok(runs),
                },
                // A blip keeps the last good answer on screen.
                Err(_) if entry.runs.is_ok() => entry,
                Err(e) => PrRuns {
                    pr: entry.pr,
                    runs: Err(e.to_string()),
                },
            }
        })
        .buffered(WORKERS)
        .collect()
        .await;
    complete(client, prs, hidden, known).await
}

async fn read_runs(client: &dyn IssueProvider, prs: Vec<LinkedPr>) -> Vec<PrRuns> {
    stream::iter(prs)
        .map(|pr| async move {
            let runs = head_runs(client, &pr).await.map_err(|e| e.to_string());
            PrRuns { pr, runs }
        })
        .buffered(WORKERS)
        .collect()
        .await
}

async fn head_runs(client: &dyn IssueProvider, pr: &LinkedPr) -> Result<Vec<ActionsRun>> {
    client
        .head_runs(&pr.pr.owner, &pr.pr.repo, &pr.head_sha)
        .await
        .map(latest_runs)
}

/// Add jobs to every run, then fetch history for active runs that lack it.
async fn complete(
    client: &dyn IssueProvider,
    mut prs: Vec<PrRuns>,
    hidden: usize,
    known: &Known,
) -> RunsUpdate {
    // Which runs need a jobs read, as (pr index, run index, run id).
    let mut needed: Vec<(usize, usize, i64)> = Vec::new();
    for (p, entry) in prs.iter_mut().enumerate() {
        let Ok(runs) = &mut entry.runs else { continue };
        for (r, run) in runs.iter_mut().enumerate() {
            match known.finished_jobs.get(&run.id) {
                Some(jobs) if !run.is_active() => run.jobs = jobs.clone(),
                _ => needed.push((p, r, run.id)),
            }
        }
    }
    let fetched: Vec<_> = {
        let prs = &prs;
        stream::iter(needed)
            .map(|(p, r, id)| async move {
                let pr = &prs[p].pr.pr;
                (p, r, client.run_jobs(&pr.owner, &pr.repo, id).await)
            })
            .buffered(WORKERS)
            .collect()
            .await
    };
    // A failed jobs read leaves the run without jobs rather than failing the
    // PR: the run's own status line is still true.
    for (p, r, jobs) in fetched {
        if let (Ok(runs), Ok(jobs)) = (&mut prs[p].runs, jobs) {
            runs[r].jobs = jobs;
        }
    }

    let mut wanted: Vec<HistoryKey> = Vec::new();
    for entry in &prs {
        let Ok(runs) = &entry.runs else { continue };
        for run in runs.iter().filter(|r| r.is_active()) {
            let key = HistoryKey::of(&entry.pr.pr, run);
            if !known.history.contains(&key) && !wanted.contains(&key) {
                wanted.push(key);
            }
        }
    }
    let history = stream::iter(wanted)
        .map(|key| async move {
            // No history is an answer too: the tab says so, and asking again
            // every poll would burn about 11 requests each time. `r` retries.
            let past = client
                .history_runs(&key.owner, &key.repo, key.workflow_id, &key.event)
                .await
                .unwrap_or_default();
            let history = Arc::new(build_history(&past));
            (key, history)
        })
        .buffered(WORKERS)
        .collect()
        .await;

    RunsUpdate {
        prs,
        hidden,
        history,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::provider::error::ProviderError;
    use crate::provider::scope::FetchScope;
    use crate::provider::types::{
        Comment, FormOptions, Issue, IssueState, NewIssueParams, RateLimitData, RepoIssues,
        RepoLabel,
    };

    /// Scripted runs, and a log of every call so tests can assert what was
    /// *not* asked for.
    #[derive(Default)]
    struct Fake {
        prs: Vec<LinkedPr>,
        runs: HashMap<String, Vec<ActionsRun>>,
        no_access: Option<String>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn log(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        fn count(&self, prefix: &str) -> usize {
            let calls = self.calls.lock().unwrap();
            calls.iter().filter(|c| c.starts_with(prefix)).count()
        }
    }

    #[async_trait]
    impl IssueProvider for Fake {
        async fn linked_prs(&self, _: &str, _: &[PrRef]) -> Result<Vec<LinkedPr>> {
            self.log("linked".into());
            Ok(self.prs.clone())
        }
        async fn head_runs(&self, _: &str, _: &str, sha: &str) -> Result<Vec<ActionsRun>> {
            self.log(format!("head {sha}"));
            if self.no_access.as_deref() == Some(sha) {
                return Err(ProviderError::Api("no access".into()));
            }
            Ok(self.runs.get(sha).cloned().unwrap_or_default())
        }
        async fn run_jobs(&self, _: &str, _: &str, run_id: i64) -> Result<Vec<ActionsJob>> {
            self.log(format!("jobs {run_id}"));
            Ok(vec![])
        }
        async fn history_runs(
            &self,
            _: &str,
            _: &str,
            id: i64,
            _: &str,
        ) -> Result<Vec<ActionsRun>> {
            self.log(format!("history {id}"));
            Err(ProviderError::Api("no history".into()))
        }

        async fn org_issues(&self, _: &str, _: &FetchScope) -> Result<Vec<RepoIssues>> {
            unimplemented!()
        }
        async fn issue(&self, _: &str) -> Result<Option<Issue>> {
            unimplemented!()
        }
        async fn repo_issues(
            &self,
            _: &str,
            _: &str,
            _: &FetchScope,
        ) -> Result<Option<RepoIssues>> {
            unimplemented!()
        }
        async fn comments(&self, _: &str) -> Result<Vec<Comment>> {
            unimplemented!()
        }
        async fn add_comment(&self, _: &str, _: &str) -> Result<()> {
            unimplemented!()
        }
        async fn update_comment(&self, _: &str, _: &str, _: &str) -> Result<()> {
            unimplemented!()
        }
        async fn set_state(&self, _: &str, _: IssueState) -> Result<Issue> {
            unimplemented!()
        }
        async fn update_title(&self, _: &str, _: &str) -> Result<Issue> {
            unimplemented!()
        }
        async fn update_body(&self, _: &str, _: &str) -> Result<Issue> {
            unimplemented!()
        }
        async fn set_assignees(&self, _: &str, _: &[String]) -> Result<Issue> {
            unimplemented!()
        }
        async fn set_labels(&self, _: &str, _: &str, _: &str, _: &[String]) -> Result<Issue> {
            unimplemented!()
        }
        async fn repo_labels(&self, _: &str, _: &str) -> Result<Vec<RepoLabel>> {
            unimplemented!()
        }
        async fn repo_form_options(&self, _: &str, _: &str) -> Result<FormOptions> {
            unimplemented!()
        }
        async fn create_issue(&self, _: &NewIssueParams) -> Result<Issue> {
            unimplemented!()
        }
        fn host(&self) -> String {
            String::new()
        }
        fn rate_limit(&self) -> Option<RateLimitData> {
            None
        }
    }

    fn linked(number: u64, sha: &str, open: bool) -> LinkedPr {
        LinkedPr {
            pr: PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number,
            },
            title: "pr".into(),
            url: format!("u/{number}"),
            head_sha: sha.into(),
            open,
            merged_at: (!open).then(chrono::Utc::now),
        }
    }

    fn run(id: i64, status: &str) -> ActionsRun {
        ActionsRun {
            id,
            workflow_id: 1,
            name: Some("CI".into()),
            event: "push".into(),
            status: status.into(),
            conclusion: (status == "completed").then(|| "success".to_string()),
            run_number: id as u64,
            run_started_at: None,
            updated_at: None,
            html_url: String::new(),
            jobs: vec![],
        }
    }

    #[tokio::test]
    async fn one_unreadable_pr_does_not_take_the_others_down() {
        let fake = Fake {
            prs: vec![linked(1, "good", true), linked(2, "bad", true)],
            runs: HashMap::from([("good".to_string(), vec![run(10, "completed")])]),
            no_access: Some("bad".into()),
            ..Fake::default()
        };
        let update = load(&fake, "I_1", &[], &Known::default()).await.unwrap();
        let by_number = |n: u64| update.prs.iter().find(|p| p.pr.pr.number == n).unwrap();
        assert!(by_number(1).runs.is_ok());
        assert_eq!(by_number(2).runs, Err("API errors: no access".to_string()));
    }

    #[tokio::test]
    async fn a_load_applies_the_pr_filter() {
        let mut closed = linked(1, "x", false);
        closed.merged_at = None;
        let fake = Fake {
            prs: vec![closed, linked(2, "y", true)],
            ..Fake::default()
        };
        let update = load(&fake, "I_1", &[], &Known::default()).await.unwrap();
        assert_eq!(update.prs.len(), 1);
        assert_eq!(update.prs[0].pr.pr.number, 2);
    }

    #[tokio::test]
    async fn finished_jobs_are_not_reread_but_active_ones_are() {
        let fake = Fake {
            prs: vec![linked(1, "s", true)],
            runs: HashMap::from([(
                "s".to_string(),
                vec![run(10, "completed"), {
                    let mut r = run(11, "in_progress");
                    r.workflow_id = 2;
                    r
                }],
            )]),
            ..Fake::default()
        };
        let known = Known {
            finished_jobs: HashMap::from([(10, vec![])]),
            ..Known::default()
        };
        load(&fake, "I_1", &[], &known).await.unwrap();
        assert_eq!(fake.count("jobs 10"), 0);
        assert_eq!(fake.count("jobs 11"), 1);
    }

    #[tokio::test]
    async fn history_is_only_asked_for_active_runs_and_only_once() {
        let fake = Fake {
            prs: vec![linked(1, "s", true)],
            runs: HashMap::from([(
                "s".to_string(),
                vec![run(10, "in_progress"), {
                    let mut r = run(11, "completed");
                    r.workflow_id = 2;
                    r
                }],
            )]),
            ..Fake::default()
        };
        let update = load(&fake, "I_1", &[], &Known::default()).await.unwrap();
        assert_eq!(fake.count("history 1"), 1);
        assert_eq!(fake.count("history 2"), 0, "finished: no ETA needed");
        assert_eq!(update.history.len(), 1, "no history is still an answer");

        let known = Known {
            history: update.history.iter().map(|(k, _)| k.clone()).collect(),
            ..Known::default()
        };
        load(&fake, "I_1", &[], &known).await.unwrap();
        assert_eq!(fake.count("history 1"), 1, "not asked again");
    }

    #[tokio::test]
    async fn a_poll_leaves_merged_prs_alone_and_never_rediscovers() {
        let fake = Fake {
            runs: HashMap::from([("open".to_string(), vec![run(10, "in_progress")])]),
            ..Fake::default()
        };
        let previous = vec![
            PrRuns {
                pr: linked(1, "open", true),
                runs: Ok(vec![]),
            },
            PrRuns {
                pr: linked(2, "merged", false),
                runs: Ok(vec![run(20, "completed")]),
            },
        ];
        let update = poll(&fake, previous, 3, &Known::default()).await;
        assert_eq!(fake.count("linked"), 0);
        assert_eq!(fake.count("head merged"), 0);
        assert_eq!(update.hidden, 3);
        assert_eq!(update.prs[0].runs.as_ref().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_poll_blip_keeps_the_last_good_answer() {
        let fake = Fake {
            no_access: Some("s".into()),
            ..Fake::default()
        };
        let previous = vec![PrRuns {
            pr: linked(1, "s", true),
            runs: Ok(vec![run(10, "in_progress")]),
        }];
        let update = poll(&fake, previous, 0, &Known::default()).await;
        assert_eq!(update.prs[0].runs.as_ref().unwrap()[0].id, 10);
    }
}
