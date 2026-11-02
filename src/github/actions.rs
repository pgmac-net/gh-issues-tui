//! Finding the PRs linked to an issue and reading their Actions runs (#222).
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

use crate::provider::error::{ProviderError, Result};
use crate::provider::types::PrRef;
use crate::runs::estimate::HISTORY_RUNS;
use crate::runs::{ActionsJob, ActionsRun, LinkedPr};

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

#[derive(Deserialize)]
struct RunsPage {
    workflow_runs: Vec<ActionsRun>,
}

#[derive(Deserialize)]
struct JobsPage {
    total_count: usize,
    jobs: Vec<ActionsJob>,
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

    pub async fn head_runs(&self, owner: &str, repo: &str, sha: &str) -> Result<Vec<ActionsRun>> {
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
        Ok(page.workflow_runs)
    }

    pub async fn run_jobs(&self, owner: &str, repo: &str, run_id: i64) -> Result<Vec<ActionsJob>> {
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
            jobs.extend(p.jobs);
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
    ) -> Result<Vec<ActionsRun>> {
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
        let runs: Vec<Result<ActionsRun>> = stream::iter(page.workflow_runs)
            .map(|mut run| async move {
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
        let run = &page.workflow_runs[0];
        assert!(run.is_active() && !run.is_failed());
        assert_eq!(run.workflow(), "CI");
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
        assert_eq!(page.jobs[0].steps.len(), 2);
        assert_eq!(page.jobs[0].steps[1].started_at, None);
    }
}
