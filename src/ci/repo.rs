//! Reading a repo's runs for the repo workflows view (#266, ADR 0014).
//!
//! [`read_pages`] lists a run of pages and adds what only some runs need: jobs
//! for active runs (the forecast and the progress need them), jobs for runs
//! the view asks for by id (ones that just finished), and ETA history not yet
//! held. Completed runs' jobs are otherwise left for [`CiProvider::run_jobs`]
//! when a run is unfolded, so a page of 100 runs costs one call, not 101.

use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream;

use super::estimate::{History, build_history};
use super::{CiProvider, CiRun, HistoryKey, RunFilter};
use crate::provider::error::Result;

/// Concurrent provider calls per stage, as in the issue tab.
const WORKERS: usize = 8;

/// One read of a span of pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagesRequest {
    pub owner: String,
    pub repo: String,
    pub filter: RunFilter,
    /// Pages `first..=last`, from 1.
    pub first: u32,
    pub last: u32,
    /// Finished runs whose jobs to read anyway: the view saw them active, so
    /// what it holds is not their final state.
    pub jobs_for: HashSet<i64>,
    /// History already held, by workflow and event.
    pub known_history: HashSet<HistoryKey>,
}

/// What a read of pages hands back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PagesUpdate {
    /// The pages' runs, newest first, each run once. Jobs are filled for
    /// active runs and those asked for.
    pub runs: Vec<CiRun>,
    /// Runs matching the filter in all.
    pub total: usize,
    pub history: Vec<(HistoryKey, Arc<History>)>,
}

pub async fn read_pages(ci: &dyn CiProvider, req: &PagesRequest) -> Result<PagesUpdate> {
    let pages: Vec<Result<super::RunPage>> = stream::iter(req.first..=req.last)
        .map(|page| ci.list_runs(&req.owner, &req.repo, &req.filter, page))
        .buffered(WORKERS)
        .collect()
        .await;

    let mut runs: Vec<CiRun> = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0;
    for page in pages {
        let page = page?;
        total = total.max(page.total);
        // A run started while the pages were read pushes the rest down one,
        // so the same run can be on two pages.
        runs.extend(page.runs.into_iter().filter(|r| seen.insert(r.id)));
    }

    let wanted: Vec<usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_active() || req.jobs_for.contains(&r.id))
        .map(|(i, _)| i)
        .collect();
    let fetched: Vec<_> = {
        let runs = &runs;
        stream::iter(wanted)
            .map(|i| async move { (i, ci.run_jobs(&req.owner, &req.repo, runs[i].id).await) })
            .buffered(WORKERS)
            .collect()
            .await
    };
    // A failed jobs read leaves the run without jobs: its own status is still
    // true, and unfolding asks again.
    for (i, jobs) in fetched {
        if let Ok(jobs) = jobs {
            runs[i].jobs = jobs;
        }
    }

    let mut keys: Vec<HistoryKey> = Vec::new();
    for run in runs.iter().filter(|r| r.is_active()) {
        let key = HistoryKey::new(&req.owner, &req.repo, run);
        if !req.known_history.contains(&key) && !keys.contains(&key) {
            keys.push(key);
        }
    }
    let history = stream::iter(keys)
        .map(|key| async move {
            // No history is an answer too, as in the issue tab: asking again
            // every poll would cost about 11 requests each time. `r` retries.
            let past = ci
                .history_runs(&key.owner, &key.repo, key.workflow_id, &key.event)
                .await
                .unwrap_or_default();
            (key, Arc::new(build_history(&past)))
        })
        .buffered(WORKERS)
        .collect()
        .await;

    Ok(PagesUpdate {
        runs,
        total,
        history,
    })
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted [`CiProvider`] that logs every call, shared by the tests of
    //! this module and the repo view's state.

    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::super::{CiCapabilities, CiJob, RunPage, RunStatus, Workflow};
    use super::*;
    use crate::provider::error::ProviderError;
    use crate::provider::types::RateLimitData;

    #[derive(Default)]
    pub struct FakeCi {
        /// Every run the repo has, newest first.
        pub runs: Vec<CiRun>,
        pub fail_list: bool,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeCi {
        fn log(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        pub fn count(&self, prefix: &str) -> usize {
            let calls = self.calls.lock().unwrap();
            calls.iter().filter(|c| c.starts_with(prefix)).count()
        }
    }

    pub fn run(id: i64, status: RunStatus) -> CiRun {
        CiRun {
            id,
            workflow_id: 1,
            name: Some("CI".into()),
            event: "push".into(),
            status,
            run_number: id as u64,
            run_attempt: 1,
            branch: Some("main".into()),
            ..CiRun::default()
        }
    }

    #[async_trait]
    impl CiProvider for FakeCi {
        fn capabilities(&self) -> CiCapabilities {
            CiCapabilities {
                status: true,
                workflow: true,
                event: true,
                branch: true,
                actor: true,
            }
        }
        async fn list_runs(
            &self,
            _: &str,
            _: &str,
            filter: &RunFilter,
            page: u32,
        ) -> Result<RunPage> {
            self.log(format!("list {page}"));
            if self.fail_list {
                return Err(ProviderError::Api("no access".into()));
            }
            let matching: Vec<CiRun> = self
                .runs
                .iter()
                .filter(|r| filter.status.is_none_or(|s| r.status == s))
                .filter(|r| {
                    filter
                        .branch
                        .as_ref()
                        .is_none_or(|b| r.branch.as_ref() == Some(b))
                })
                .cloned()
                .collect();
            let size = 2;
            let start = (page as usize - 1) * size;
            Ok(RunPage {
                total: matching.len(),
                runs: matching.into_iter().skip(start).take(size).collect(),
            })
        }
        async fn list_workflows(&self, _: &str, _: &str) -> Result<Vec<Workflow>> {
            self.log("workflows".into());
            Ok(vec![Workflow {
                id: 1,
                name: "CI".into(),
            }])
        }
        async fn head_runs(&self, _: &str, _: &str, _: &str) -> Result<Vec<CiRun>> {
            unimplemented!()
        }
        async fn run_jobs(&self, _: &str, _: &str, run_id: i64) -> Result<Vec<CiJob>> {
            self.log(format!("jobs {run_id}"));
            Ok(vec![CiJob {
                id: run_id * 10,
                name: "build".into(),
                status: RunStatus::Success,
                started_at: None,
                completed_at: None,
                html_url: None,
                steps: vec![],
            }])
        }
        async fn history_runs(&self, _: &str, _: &str, id: i64, _: &str) -> Result<Vec<CiRun>> {
            self.log(format!("history {id}"));
            Ok(vec![])
        }
        fn rate_limit(&self) -> Option<RateLimitData> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RunStatus;
    use super::fake::{FakeCi, run};
    use super::*;

    fn req(first: u32, last: u32) -> PagesRequest {
        PagesRequest {
            owner: "o".into(),
            repo: "r".into(),
            filter: RunFilter::default(),
            first,
            last,
            jobs_for: HashSet::new(),
            known_history: HashSet::new(),
        }
    }

    #[tokio::test]
    async fn only_active_runs_get_jobs_and_history() {
        let ci = FakeCi {
            runs: vec![run(4, RunStatus::Running), run(3, RunStatus::Success)],
            ..FakeCi::default()
        };
        let update = read_pages(&ci, &req(1, 1)).await.unwrap();
        assert_eq!(update.runs.len(), 2);
        assert_eq!(ci.count("jobs 4"), 1);
        assert_eq!(
            ci.count("jobs 3"),
            0,
            "finished runs' jobs wait for an unfold"
        );
        assert_eq!(ci.count("history"), 1);
        assert!(!update.runs[0].jobs.is_empty() && update.runs[1].jobs.is_empty());
    }

    #[tokio::test]
    async fn a_run_that_just_finished_has_its_jobs_reread() {
        let ci = FakeCi {
            runs: vec![run(3, RunStatus::Success)],
            ..FakeCi::default()
        };
        let mut r = req(1, 1);
        r.jobs_for.insert(3);
        let update = read_pages(&ci, &r).await.unwrap();
        assert_eq!(ci.count("jobs 3"), 1);
        assert!(!update.runs[0].jobs.is_empty());
    }

    #[tokio::test]
    async fn every_page_in_the_span_is_read_and_a_run_shows_once() {
        let ci = FakeCi {
            runs: (1..=5)
                .rev()
                .map(|id| run(id, RunStatus::Success))
                .collect(),
            ..FakeCi::default()
        };
        let update = read_pages(&ci, &req(1, 3)).await.unwrap();
        assert_eq!(ci.count("list"), 3);
        assert_eq!(update.total, 5);
        let ids: Vec<i64> = update.runs.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![5, 4, 3, 2, 1]);
    }

    #[tokio::test]
    async fn known_history_is_not_asked_for_again() {
        let ci = FakeCi {
            runs: vec![run(4, RunStatus::Running)],
            ..FakeCi::default()
        };
        let mut r = req(1, 1);
        r.known_history
            .insert(HistoryKey::new("o", "r", &run(4, RunStatus::Running)));
        read_pages(&ci, &r).await.unwrap();
        assert_eq!(ci.count("history"), 0);
    }

    #[tokio::test]
    async fn a_failed_list_fails_the_read() {
        let ci = FakeCi {
            fail_list: true,
            ..FakeCi::default()
        };
        assert!(read_pages(&ci, &req(1, 1)).await.is_err());
    }
}
