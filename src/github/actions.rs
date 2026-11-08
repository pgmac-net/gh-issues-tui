//! Finding the PRs linked to an issue (#222), and GitHub Actions as a
//! [`CiProvider`] (#266).
//!
//! The PRs come over GraphQL; the runs, jobs and steps only exist on REST.
//! REST draws on a different rate-limit budget from GraphQL, so it keeps its
//! own store: sharing one would make the info bar's number flip between two
//! unrelated budgets on every poll.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::stream;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use async_trait::async_trait;

use crate::ci::estimate::HISTORY_RUNS;
use crate::ci::{
    CiCapabilities, CiJob, CiProvider, CiRun, CiStep, RunFilter, RunPage, RunStatus, Workflow,
};
use crate::provider::error::{ProviderError, Result};
use crate::provider::types::{PrRef, RateLimitData};
use crate::runs::LinkedPr;

use super::Client;

const REST_URL: &str = "https://api.github.com";
const REST_PAGE: usize = 100;
/// Text references resolved per issue: each costs a request, and a thread
/// quoting dozens of PRs is not linked to dozens of PRs.
const MAX_TEXT_REFS: usize = 20;
/// Concurrent requests, gh-eta's `fetchWorkers`.
const WORKERS: usize = 8;

const LINKED_PR_FIELDS: &str = "
fragment LinkedPrFields on PullRequest {
  number title url state mergedAt headRefOid
  repository { name owner { login } }
}";

/// Every PR the issue's timeline knows about. `Closes #N` in a PR body shows up
/// here as a cross-reference too, so closing references need no second query.
const TIMELINE_QUERY: &str = "
query($id: ID!) {
  node(id: $id) {
    ... on Issue {
      timelineItems(last: 100, itemTypes: [CROSS_REFERENCED_EVENT, CONNECTED_EVENT]) {
        nodes {
          ... on CrossReferencedEvent { source { ...LinkedPrFields } }
          ... on ConnectedEvent { subject { ...LinkedPrFields } }
        }
      }
    }
  }
}";

const TEXT_REF_QUERY: &str = "
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issueOrPullRequest(number: $number) { ...LinkedPrFields }
  }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    number: u64,
    title: String,
    url: String,
    state: String,
    merged_at: Option<DateTime<Utc>>,
    head_ref_oid: String,
    repository: PrRepo,
}

#[derive(Deserialize)]
struct PrRepo {
    name: String,
    owner: PrOwner,
}

#[derive(Deserialize)]
struct PrOwner {
    login: String,
}

impl From<PrNode> for LinkedPr {
    fn from(n: PrNode) -> Self {
        LinkedPr {
            pr: PrRef {
                owner: n.repository.owner.login,
                repo: n.repository.name,
                number: n.number,
            },
            title: n.title,
            url: n.url,
            head_sha: n.head_ref_oid,
            open: n.state == "OPEN",
            merged_at: n.merged_at,
        }
    }
}

/// A union member that is not a PR arrives as `{}`, so anything that does not
/// parse as one is simply not a PR.
fn as_linked_pr(v: &Value) -> Option<LinkedPr> {
    serde_json::from_value::<PrNode>(v.clone())
        .ok()
        .map(LinkedPr::from)
}

/// The PRs in a timeline response.
fn timeline_prs(data: &Value) -> Vec<LinkedPr> {
    data.pointer("/node/timelineItems/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| n.get("source").or_else(|| n.get("subject")))
        .filter_map(as_linked_pr)
        .collect()
}

fn same_pr(a: &PrRef, b: &PrRef) -> bool {
    a.number == b.number
        && a.owner.eq_ignore_ascii_case(&b.owner)
        && a.repo.eq_ignore_ascii_case(&b.repo)
}

/// GitHub's two words for where something stands, as one [`RunStatus`]. The
/// only place either word is read (ADR 0014).
pub(crate) fn gh_status(status: &str, conclusion: Option<&str>) -> RunStatus {
    match (status, conclusion) {
        ("completed", Some("success")) => RunStatus::Success,
        ("completed", Some("failure")) => RunStatus::Failure,
        ("completed", Some("cancelled")) => RunStatus::Cancelled,
        ("completed", Some("timed_out")) => RunStatus::TimedOut,
        ("completed", Some("action_required")) => RunStatus::ActionRequired,
        ("completed", Some("startup_failure")) => RunStatus::StartupFailure,
        ("completed", Some("skipped")) => RunStatus::Skipped,
        ("completed", Some("stale")) => RunStatus::Stale,
        // `neutral`, a conclusion GitHub adds later, or none at all.
        ("completed", _) => RunStatus::Neutral,
        ("in_progress", _) => RunStatus::Running,
        ("waiting", _) => RunStatus::Waiting,
        // queued, requested, pending, and anything new that has not started.
        _ => RunStatus::Queued,
    }
}

/// The `status` query value that narrows a run list to `s`. `startup_failure`
/// is not one GitHub accepts, so it asks for every failure and the client
/// narrows: the filter is applied on both sides.
fn gh_status_param(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "in_progress",
        RunStatus::StartupFailure => "failure",
        other => other.word(),
    }
}

#[derive(Deserialize)]
struct GhStep {
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

impl From<GhStep> for CiStep {
    fn from(s: GhStep) -> Self {
        CiStep {
            status: gh_status(&s.status, s.conclusion.as_deref()),
            name: s.name,
            started_at: s.started_at,
            completed_at: s.completed_at,
        }
    }
}

#[derive(Deserialize)]
struct GhJob {
    id: i64,
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    steps: Vec<GhStep>,
}

impl From<GhJob> for CiJob {
    fn from(j: GhJob) -> Self {
        CiJob {
            status: gh_status(&j.status, j.conclusion.as_deref()),
            id: j.id,
            name: j.name,
            started_at: j.started_at,
            completed_at: j.completed_at,
            html_url: j.html_url,
            steps: j.steps.into_iter().map(CiStep::from).collect(),
        }
    }
}

#[derive(Deserialize)]
struct GhActor {
    login: String,
}

#[derive(Deserialize)]
struct GhRun {
    id: i64,
    workflow_id: i64,
    #[serde(default)]
    name: Option<String>,
    event: String,
    status: String,
    conclusion: Option<String>,
    run_number: u64,
    #[serde(default = "first_attempt")]
    run_attempt: u32,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    run_started_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    html_url: String,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    display_title: Option<String>,
    #[serde(default)]
    actor: Option<GhActor>,
}

fn first_attempt() -> u32 {
    1
}

impl From<GhRun> for CiRun {
    fn from(r: GhRun) -> Self {
        CiRun {
            status: gh_status(&r.status, r.conclusion.as_deref()),
            id: r.id,
            workflow_id: r.workflow_id,
            name: r.name,
            event: r.event,
            run_number: r.run_number,
            run_attempt: r.run_attempt,
            created_at: r.created_at,
            run_started_at: r.run_started_at,
            updated_at: r.updated_at,
            html_url: r.html_url,
            branch: r.head_branch,
            title: r.display_title,
            actor: r.actor.map(|a| a.login),
            jobs: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct RunsPage {
    #[serde(default)]
    total_count: usize,
    workflow_runs: Vec<GhRun>,
}

#[derive(Deserialize)]
struct JobsPage {
    total_count: usize,
    jobs: Vec<GhJob>,
}

#[derive(Deserialize)]
struct GhWorkflow {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct WorkflowsPage {
    total_count: usize,
    workflows: Vec<GhWorkflow>,
}

/// The query a run list sends for `filter`, page `page`. The workflow is not
/// here: it picks the endpoint instead.
fn list_query(filter: &RunFilter, page: u32) -> Vec<(&'static str, String)> {
    let mut q = vec![
        ("per_page", REST_PAGE.to_string()),
        ("page", page.to_string()),
        ("exclude_pull_requests", "true".to_string()),
    ];
    if let Some(s) = filter.status {
        q.push(("status", gh_status_param(s).to_string()));
    }
    for (key, value) in [
        ("event", &filter.event),
        ("branch", &filter.branch),
        ("actor", &filter.actor),
    ] {
        if let Some(v) = value {
            q.push((key, v.clone()));
        }
    }
    q
}

impl Client {
    pub async fn linked_prs(&self, issue_id: &str, text_refs: &[PrRef]) -> Result<Vec<LinkedPr>> {
        let query = format!("{TIMELINE_QUERY}{LINKED_PR_FIELDS}");
        let data = self.graphql(&query, json!({ "id": issue_id })).await?;

        let mut prs: Vec<LinkedPr> = Vec::new();
        let mut seen = HashSet::new();
        for pr in timeline_prs(&data) {
            if seen.insert(pr.url.clone()) {
                prs.push(pr);
            }
        }

        let unknown: Vec<PrRef> = text_refs
            .iter()
            .filter(|r| !prs.iter().any(|p| same_pr(&p.pr, r)))
            .take(MAX_TEXT_REFS)
            .cloned()
            .collect();
        let resolved: Vec<Result<Option<LinkedPr>>> = stream::iter(unknown)
            .map(|r| async move { self.text_ref(&r).await })
            .buffered(WORKERS)
            .collect()
            .await;
        for result in resolved {
            match result {
                Ok(Some(pr)) if seen.insert(pr.url.clone()) => prs.push(pr),
                Ok(_) => {}
                Err(e @ ProviderError::RateLimited(_)) => return Err(e),
                // Text is free-form: `#12` in a repo that has no #12, or a repo
                // this token cannot see, is not a failure of the timeline's
                // answer. GitHub answers those two identically.
                Err(_) => {}
            }
        }
        Ok(prs)
    }

    /// Resolve a reference found in issue text. `None` when it names an issue.
    async fn text_ref(&self, r: &PrRef) -> Result<Option<LinkedPr>> {
        let query = format!("{TEXT_REF_QUERY}{LINKED_PR_FIELDS}");
        let data = self
            .graphql(
                &query,
                json!({ "owner": r.owner, "name": r.repo, "number": r.number }),
            )
            .await?;
        Ok(data
            .pointer("/repository/issueOrPullRequest")
            .and_then(as_linked_pr))
    }

    pub async fn head_runs(&self, owner: &str, repo: &str, sha: &str) -> Result<Vec<CiRun>> {
        let page: RunsPage = self
            .rest_get(
                &format!("repos/{owner}/{repo}/actions/runs"),
                &[
                    ("head_sha", sha.to_string()),
                    ("per_page", REST_PAGE.to_string()),
                    ("exclude_pull_requests", "true".to_string()),
                ],
            )
            .await?;
        Ok(page.workflow_runs.into_iter().map(CiRun::from).collect())
    }

    pub async fn list_runs(
        &self,
        owner: &str,
        repo: &str,
        filter: &RunFilter,
        page: u32,
    ) -> Result<RunPage> {
        let path = match &filter.workflow {
            Some(w) => format!("repos/{owner}/{repo}/actions/workflows/{}/runs", w.id),
            None => format!("repos/{owner}/{repo}/actions/runs"),
        };
        let p: RunsPage = self.rest_get(&path, &list_query(filter, page)).await?;
        Ok(RunPage {
            total: p.total_count,
            runs: p.workflow_runs.into_iter().map(CiRun::from).collect(),
        })
    }

    pub async fn list_workflows(&self, owner: &str, repo: &str) -> Result<Vec<Workflow>> {
        let path = format!("repos/{owner}/{repo}/actions/workflows");
        let mut workflows = Vec::new();
        for page in 1.. {
            let p: WorkflowsPage = self
                .rest_get(
                    &path,
                    &[
                        ("per_page", REST_PAGE.to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let last = p.workflows.is_empty();
            workflows.extend(p.workflows.into_iter().map(|w| Workflow {
                id: w.id,
                name: w.name,
            }));
            if last || workflows.len() >= p.total_count {
                break;
            }
        }
        workflows.sort_by_key(|w| w.name.to_lowercase());
        Ok(workflows)
    }

    pub async fn run_jobs(&self, owner: &str, repo: &str, run_id: i64) -> Result<Vec<CiJob>> {
        let path = format!("repos/{owner}/{repo}/actions/runs/{run_id}/jobs");
        let mut jobs = Vec::new();
        for page in 1.. {
            let p: JobsPage = self
                .rest_get(
                    &path,
                    &[
                        ("per_page", REST_PAGE.to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let last = p.jobs.is_empty();
            jobs.extend(p.jobs.into_iter().map(CiJob::from));
            if last || jobs.len() >= p.total_count {
                break;
            }
        }
        Ok(jobs)
    }

    pub async fn history_runs(
        &self,
        owner: &str,
        repo: &str,
        workflow_id: i64,
        event: &str,
    ) -> Result<Vec<CiRun>> {
        let page: RunsPage = self
            .rest_get(
                &format!("repos/{owner}/{repo}/actions/workflows/{workflow_id}/runs"),
                &[
                    ("status", "success".to_string()),
                    ("exclude_pull_requests", "true".to_string()),
                    ("per_page", HISTORY_RUNS.to_string()),
                    ("event", event.to_string()),
                ],
            )
            .await?;
        let runs: Vec<Result<CiRun>> = stream::iter(page.workflow_runs)
            .map(|run| async move {
                let mut run = CiRun::from(run);
                run.jobs = self.run_jobs(owner, repo, run.id).await?;
                Ok(run)
            })
            .buffered(WORKERS)
            .collect()
            .await;
        runs.into_iter().collect()
    }

    async fn rest_get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = reqwest::Url::parse_with_params(&format!("{REST_URL}/{path}"), query)
            .map_err(|e| ProviderError::Shape(format!("bad Actions URL: {e}")))?;
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        self.note_rate_limit(&self.rest_rate_limit, resp.headers());

        let status = resp.status();
        let throttled = status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || (status == reqwest::StatusCode::FORBIDDEN
                && (resp.headers().contains_key(reqwest::header::RETRY_AFTER)
                    || self.rest_rate_limit.get().is_some_and(|d| d.remaining == 0)));
        if throttled {
            return Err(ProviderError::RateLimited(
                self.rest_rate_limit.message(Some("REST")),
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            return Err(ProviderError::Api(format!(
                "HTTP {} reading Actions: the token needs Actions read on this repo",
                status.as_u16()
            )));
        }
        Ok(resp.error_for_status()?.json().await?)
    }
}

#[async_trait]
impl CiProvider for Client {
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
        owner: &str,
        repo: &str,
        filter: &RunFilter,
        page: u32,
    ) -> Result<RunPage> {
        Client::list_runs(self, owner, repo, filter, page).await
    }

    async fn list_workflows(&self, owner: &str, repo: &str) -> Result<Vec<Workflow>> {
        Client::list_workflows(self, owner, repo).await
    }

    async fn head_runs(&self, owner: &str, repo: &str, sha: &str) -> Result<Vec<CiRun>> {
        Client::head_runs(self, owner, repo, sha).await
    }

    async fn run_jobs(&self, owner: &str, repo: &str, run_id: i64) -> Result<Vec<CiJob>> {
        Client::run_jobs(self, owner, repo, run_id).await
    }

    async fn history_runs(
        &self,
        owner: &str,
        repo: &str,
        workflow_id: i64,
        event: &str,
    ) -> Result<Vec<CiRun>> {
        Client::history_runs(self, owner, repo, workflow_id, event).await
    }

    fn rate_limit(&self) -> Option<RateLimitData> {
        self.rest_rate_limit.get()
    }

    fn runs_page_url(&self, owner: &str, repo: &str, filter: &RunFilter) -> Option<String> {
        Some(actions_page_url(owner, repo, filter))
    }
}

/// The Actions tab, narrowed with its own search syntax.
fn actions_page_url(owner: &str, repo: &str, filter: &RunFilter) -> String {
    let base = format!("https://github.com/{owner}/{repo}/actions");
    let mut terms = Vec::new();
    if let Some(w) = &filter.workflow {
        terms.push(format!("workflow:\"{}\"", w.name));
    }
    if let Some(s) = filter.status {
        terms.push(format!("is:{}", gh_status_param(s)));
    }
    for (key, value) in [
        ("event", &filter.event),
        ("branch", &filter.branch),
        ("actor", &filter.actor),
    ] {
        if let Some(v) = value {
            terms.push(format!("{key}:{v}"));
        }
    }
    if terms.is_empty() {
        return base;
    }
    reqwest::Url::parse_with_params(&base, &[("query", terms.join(" "))])
        .map_or(base, |u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_node(number: u64, state: &str, merged_at: Value) -> Value {
        json!({
            "number": number,
            "title": "feat: runs",
            "url": format!("https://github.com/o/r/pull/{number}"),
            "state": state,
            "mergedAt": merged_at,
            "headRefOid": "abc123",
            "repository": { "name": "r", "owner": { "login": "o" } },
        })
    }

    #[test]
    fn timeline_keeps_prs_from_both_event_kinds_and_skips_issues() {
        let data = json!({ "node": { "timelineItems": { "nodes": [
            { "source": pr_node(1, "OPEN", Value::Null) },
            { "source": {} },
            { "subject": pr_node(2, "MERGED", json!("2026-10-30T01:02:03Z")) },
            {},
        ]}}});
        let prs = timeline_prs(&data);
        let numbers: Vec<u64> = prs.iter().map(|p| p.pr.number).collect();
        assert_eq!(numbers, vec![1, 2]);
        assert!(prs[0].open && !prs[0].merged());
        assert!(!prs[1].open && prs[1].merged());
        assert_eq!(prs[0].pr.owner, "o");
        assert_eq!(prs[0].head_sha, "abc123");
    }

    #[test]
    fn a_timeline_with_no_issue_behind_it_is_empty_not_an_error() {
        assert!(timeline_prs(&json!({ "node": null })).is_empty());
    }

    #[test]
    fn same_pr_ignores_case_in_owner_and_repo() {
        let a = PrRef {
            owner: "PGMac-Net".into(),
            repo: "Repo".into(),
            number: 3,
        };
        let b = PrRef {
            owner: "pgmac-net".into(),
            repo: "repo".into(),
            number: 3,
        };
        assert!(same_pr(&a, &b));
    }

    #[test]
    fn a_runs_page_parses_with_nulls_where_a_run_is_still_going() {
        let page: RunsPage = serde_json::from_value(json!({
            "total_count": 1,
            "workflow_runs": [{
                "id": 9,
                "workflow_id": 7,
                "name": "CI",
                "event": "pull_request",
                "status": "in_progress",
                "conclusion": null,
                "run_number": 42,
                "run_started_at": "2026-10-30T01:02:03Z",
                "updated_at": "2026-10-30T01:03:03Z",
                "html_url": "https://github.com/o/r/actions/runs/9",
            }],
        }))
        .unwrap();
        assert_eq!(page.total_count, 1);
        let run = CiRun::from(page.workflow_runs.into_iter().next().unwrap());
        assert!(run.is_active() && !run.is_failed());
        assert_eq!(run.status, RunStatus::Running);
        assert_eq!(run.workflow(), "CI");
        assert_eq!(run.run_attempt, 1, "a missing attempt is the first");
        assert!(run.jobs.is_empty());
    }

    #[test]
    fn a_jobs_page_parses_steps_that_have_not_started() {
        let page: JobsPage = serde_json::from_value(json!({
            "total_count": 1,
            "jobs": [{
                "id": 5,
                "name": "test",
                "status": "in_progress",
                "conclusion": null,
                "started_at": "2026-10-30T01:02:03Z",
                "completed_at": null,
                "html_url": "https://github.com/o/r/actions/runs/9/job/5",
                "steps": [
                    { "name": "checkout", "number": 1, "status": "completed",
                      "conclusion": "success",
                      "started_at": "2026-10-30T01:02:03Z",
                      "completed_at": "2026-10-30T01:02:05Z" },
                    { "name": "cargo test", "number": 2, "status": "pending",
                      "conclusion": null, "started_at": null, "completed_at": null },
                ],
            }],
        }))
        .unwrap();
        let job = CiJob::from(page.jobs.into_iter().next().unwrap());
        assert_eq!(job.steps.len(), 2);
        assert_eq!(job.steps[0].status, RunStatus::Success);
        assert_eq!(job.steps[1].status, RunStatus::Queued, "pending is queued");
        assert_eq!(job.steps[1].started_at, None);
    }

    #[test]
    fn a_runs_page_carries_branch_title_actor_and_attempt() {
        let page: RunsPage = serde_json::from_value(json!({
            "total_count": 431,
            "workflow_runs": [{
                "id": 9, "workflow_id": 7, "name": "Release", "event": "push",
                "status": "completed", "conclusion": "startup_failure",
                "run_number": 88, "run_attempt": 2,
                "created_at": "2026-10-30T01:00:00Z",
                "run_started_at": "2026-10-30T01:02:03Z",
                "updated_at": "2026-10-30T01:03:03Z",
                "html_url": "https://github.com/o/r/actions/runs/9",
                "head_branch": "main",
                "display_title": "fix: something",
                "actor": { "login": "pgmac" },
            }],
        }))
        .unwrap();
        assert_eq!(page.total_count, 431);
        let run = CiRun::from(page.workflow_runs.into_iter().next().unwrap());
        assert_eq!(run.status, RunStatus::StartupFailure);
        assert_eq!(run.run_attempt, 2);
        assert_eq!(run.branch.as_deref(), Some("main"));
        assert_eq!(run.title.as_deref(), Some("fix: something"));
        assert_eq!(run.actor.as_deref(), Some("pgmac"));
    }

    #[test]
    fn a_group_header_opens_the_actions_tab_narrowed_to_it() {
        assert_eq!(
            actions_page_url("o", "r", &RunFilter::default()),
            "https://github.com/o/r/actions"
        );
        let f = RunFilter {
            status: Some(RunStatus::Failure),
            branch: Some("main".into()),
            ..RunFilter::default()
        };
        assert_eq!(
            actions_page_url("o", "r", &f),
            "https://github.com/o/r/actions?query=is%3Afailure+branch%3Amain"
        );
    }

    #[test]
    fn every_github_word_pair_maps_to_a_status() {
        let cases = [
            ("completed", Some("success"), RunStatus::Success),
            ("completed", Some("failure"), RunStatus::Failure),
            ("completed", Some("cancelled"), RunStatus::Cancelled),
            ("completed", Some("timed_out"), RunStatus::TimedOut),
            (
                "completed",
                Some("action_required"),
                RunStatus::ActionRequired,
            ),
            (
                "completed",
                Some("startup_failure"),
                RunStatus::StartupFailure,
            ),
            ("completed", Some("skipped"), RunStatus::Skipped),
            ("completed", Some("stale"), RunStatus::Stale),
            ("completed", Some("neutral"), RunStatus::Neutral),
            ("completed", None, RunStatus::Neutral),
            ("in_progress", None, RunStatus::Running),
            ("waiting", None, RunStatus::Waiting),
            ("queued", None, RunStatus::Queued),
            ("requested", None, RunStatus::Queued),
            ("pending", None, RunStatus::Queued),
        ];
        for (status, conclusion, want) in cases {
            assert_eq!(
                gh_status(status, conclusion),
                want,
                "{status}/{conclusion:?}"
            );
        }
    }

    #[test]
    fn a_filter_becomes_query_params_github_accepts() {
        let filter = RunFilter {
            status: Some(RunStatus::Running),
            branch: Some("main".into()),
            actor: Some("pgmac".into()),
            ..RunFilter::default()
        };
        let q = list_query(&filter, 3);
        let get = |k: &str| q.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("status"), Some("in_progress"));
        assert_eq!(get("branch"), Some("main"));
        assert_eq!(get("actor"), Some("pgmac"));
        assert_eq!(get("page"), Some("3"));
        assert_eq!(get("event"), None);
        assert_eq!(gh_status_param(RunStatus::StartupFailure), "failure");
        for s in RunStatus::ALL {
            assert_ne!(
                gh_status_param(s),
                "running",
                "GitHub has no `running` status"
            );
        }
    }
}
