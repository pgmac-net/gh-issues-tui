use base64::Engine;
use serde_json::{Value, json};

use super::auth::JiraCreds;
use super::{
    adf_to_text, key_to_number, priority_name_to_value, priority_value_to_name,
    synthetic_priority_id_to_name, text_to_adf,
};
use crate::provider::error::{ProviderError, RATE_LIMIT_MSG_PREFIX, Result};
use crate::provider::types::{
    Comment, FormOptions, IdName, Issue, IssueState, Label, NewIssueParams, RateLimitData,
    RepoIssues, RepoLabel,
};

/// Fields requested for every issue in the list fetch.
const ISSUE_FIELDS: &str = "summary,description,status,assignee,reporter,priority,labels,comment,created,updated,resolutiondate,issuetype";
const PROJECTS_PAGE: u32 = 50;
const ISSUES_PAGE: u32 = 50;

/// Async Jira Cloud REST client. Cheap to clone (one `reqwest::Client`).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn new(creds: JiraCreds) -> anyhow::Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        // Jira Cloud uses HTTP Basic auth: base64(email:api_token).
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", creds.email, creds.token));
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Basic {basic}"))?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        let http = reqwest::Client::builder()
            .user_agent(concat!("gh-issues/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .build()?;
        Ok(Self {
            http,
            base_url: creds.base_url,
        })
    }

    pub fn rate_limit(&self) -> Option<RateLimitData> {
        // Jira Cloud does not report remaining/limit on normal responses; the
        // status line simply omits a counter for this provider.
        None
    }

    fn url(&self, path: &str) -> String {
        format!("{}/rest/api/3{path}", self.base_url)
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = reqwest::Url::parse_with_params(&self.url(path), query)
            .map_err(|e| ProviderError::Shape(e.to_string()))?;
        let resp = self.http.get(url).send().await?;
        self.read(resp).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let resp = self.http.post(self.url(path)).json(&body).send().await?;
        self.read(resp).await
    }

    async fn put(&self, path: &str, body: Value) -> Result<()> {
        let resp = self.http.put(self.url(path)).json(&body).send().await?;
        self.read(resp).await.map(drop)
    }

    /// Read a response: classify rate-limit / error status, then parse JSON (an
    /// empty 2xx body — common for PUT/transition — yields `Value::Null`).
    async fn read(&self, resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| format!(" (retry after {s}s)"))
                .unwrap_or_default();
            return Err(ProviderError::RateLimited(format!(
                "{RATE_LIMIT_MSG_PREFIX} (Jira){retry}"
            )));
        }
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(ProviderError::Api(parse_jira_errors(&text, status)));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| ProviderError::Shape(e.to_string()))
    }

    /// Fetch every project as a group, each with its issues. `org` is ignored —
    /// the site is fixed by the credentials. `include_closed` keeps Done issues;
    /// otherwise they are filtered out via JQL.
    pub async fn org_issues(&self, _org: &str, include_closed: bool) -> Result<Vec<RepoIssues>> {
        // Phase 1: list projects (paged).
        let mut projects: Vec<ProjectRef> = Vec::new();
        let mut start = 0u32;
        loop {
            let data = self
                .get(
                    "/project/search",
                    &[
                        ("startAt", start.to_string()),
                        ("maxResults", PROJECTS_PAGE.to_string()),
                    ],
                )
                .await?;
            let values = data
                .get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| ProviderError::Shape("project search: missing values".into()))?;
            for v in values {
                projects.push(ProjectRef {
                    key: v
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            if data.get("isLast").and_then(Value::as_bool).unwrap_or(true) {
                break;
            }
            start += PROJECTS_PAGE;
        }

        // Phase 2: page each project's issues.
        let mut out = Vec::new();
        for project in projects {
            let issues = self.project_issues(&project.key, include_closed).await?;
            out.push(RepoIssues {
                repo: project.key.clone(),
                repo_url: format!("{}/browse/{}", self.base_url, project.key),
                issues,
            });
        }
        Ok(out)
    }

    async fn project_issues(&self, key: &str, include_closed: bool) -> Result<Vec<Issue>> {
        let mut jql = format!("project = \"{key}\"");
        if !include_closed {
            jql.push_str(" AND statusCategory != Done");
        }
        jql.push_str(" ORDER BY updated DESC");

        let mut issues = Vec::new();
        let mut start = 0u32;
        loop {
            let data = self
                .get(
                    "/search",
                    &[
                        ("jql", jql.clone()),
                        ("startAt", start.to_string()),
                        ("maxResults", ISSUES_PAGE.to_string()),
                        ("fields", ISSUE_FIELDS.to_string()),
                    ],
                )
                .await?;
            let batch = data
                .get("issues")
                .and_then(Value::as_array)
                .ok_or_else(|| ProviderError::Shape("issue search: missing issues".into()))?;
            for raw in batch {
                issues.push(map_issue(raw));
            }
            let total = data.get("total").and_then(Value::as_u64).unwrap_or(0);
            start += ISSUES_PAGE;
            if u64::from(start) >= total || batch.is_empty() {
                break;
            }
        }
        Ok(issues)
    }

    pub async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        let data = self.get(&format!("/issue/{issue_id}/comment"), &[]).await?;
        let nodes = data
            .get("comments")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Shape("missing comments".into()))?;
        Ok(nodes
            .iter()
            .map(|c| Comment {
                id: c
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                author: c
                    .pointer("/author/displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                created_at: c
                    .get("created")
                    .and_then(Value::as_str)
                    .and_then(super::parse_jira_dt)
                    .unwrap_or_else(chrono::Utc::now),
                body: c.get("body").map(adf_to_text).unwrap_or_default(),
            })
            .collect())
    }

    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()> {
        self.post(
            &format!("/issue/{issue_id}/comment"),
            json!({ "body": text_to_adf(body) }),
        )
        .await
        .map(drop)
    }

    /// Edit a comment. Jira addresses it by issue key + comment id.
    pub async fn update_comment(&self, issue_id: &str, comment_id: &str, body: &str) -> Result<()> {
        self.put(
            &format!("/issue/{issue_id}/comment/{comment_id}"),
            json!({ "body": text_to_adf(body) }),
        )
        .await
    }

    /// Edit an issue's description. Jira's body field is `description` (ADF).
    pub async fn update_body(&self, issue_id: &str, body: &str) -> Result<()> {
        self.put(
            &format!("/issue/{issue_id}"),
            json!({ "fields": { "description": text_to_adf(body) } }),
        )
        .await
    }

    /// Move the issue through a workflow transition whose target status
    /// category matches the wanted open/closed state. Transitions are
    /// workflow-defined per issue, so the available set is fetched first.
    pub async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()> {
        let data = self
            .get(&format!("/issue/{issue_id}/transitions"), &[])
            .await?;
        let transitions = data
            .get("transitions")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Shape("missing transitions".into()))?;
        let want_closed = matches!(state, IssueState::Closed);
        let target = transitions
            .iter()
            .find(|t| {
                let cat = t
                    .pointer("/to/statusCategory/key")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                (cat == "done") == want_closed
            })
            .and_then(|t| t.get("id").and_then(Value::as_str))
            .ok_or_else(|| {
                ProviderError::Api(format!(
                    "no workflow transition to a {} status is available for {issue_id}",
                    if want_closed { "done" } else { "not-done" }
                ))
            })?;
        self.post(
            &format!("/issue/{issue_id}/transitions"),
            json!({ "transition": { "id": target } }),
        )
        .await
        .map(drop)
    }

    pub async fn update_title(&self, issue_id: &str, title: &str) -> Result<()> {
        self.put(
            &format!("/issue/{issue_id}"),
            json!({ "fields": { "summary": title } }),
        )
        .await
    }

    /// Jira issues have a single assignee. Only the first login is used; an
    /// empty list unassigns.
    pub async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<()> {
        let account_id = match logins.first() {
            Some(login) => Some(self.resolve_account_id(login).await?),
            None => None,
        };
        self.put(
            &format!("/issue/{issue_id}/assignee"),
            json!({ "accountId": account_id }),
        )
        .await
    }

    async fn resolve_account_id(&self, query: &str) -> Result<String> {
        let data = self
            .get("/user/search", &[("query", query.to_string())])
            .await?;
        data.as_array()
            .and_then(|arr| arr.first())
            .and_then(|u| u.get("accountId"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ProviderError::Shape(format!("unknown user {query}")))
    }

    /// Replace the issue's labels. A `priority:*` name is peeled off and routed
    /// to Jira's native priority field; the rest are sent verbatim (Jira labels
    /// are free-form strings — no id resolution).
    pub async fn set_labels(
        &self,
        issue_id: &str,
        _repo: &str,
        _org: &str,
        names: &[String],
    ) -> Result<()> {
        let mut priority: Option<&str> = None;
        let mut labels: Vec<&String> = Vec::new();
        for name in names {
            match crate::provider::types::priority_value(name) {
                Some(value) => priority = priority_value_to_name(value),
                None => labels.push(name),
            }
        }
        let mut fields = json!({ "labels": labels });
        if let Some(p) = priority {
            fields["priority"] = json!({ "name": p });
        }
        self.put(&format!("/issue/{issue_id}"), json!({ "fields": fields }))
            .await
    }

    /// Global labels (best-effort) plus the synthetic `priority:*` labels so the
    /// priority picker (`p`) and label editor (`l`) have entries to show.
    pub async fn repo_labels(&self, _org: &str, _repo: &str) -> Result<Vec<RepoLabel>> {
        let mut labels: Vec<RepoLabel> = match self.get("/label", &[]).await {
            Ok(data) => data
                .get("values")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|name| RepoLabel {
                            id: name.to_string(),
                            name: name.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            // The global label endpoint is not fatal to the picker.
            Err(_) => Vec::new(),
        };
        for (id, name) in super::synthetic_priority_labels() {
            labels.push(RepoLabel { id, name });
        }
        Ok(labels)
    }

    /// New-issue form options for a project: project id, labels + synthetic
    /// priority labels, assignable users, and **issue types** (required by Jira
    /// on create). Milestones and GitHub-style projects have no equivalent.
    pub async fn repo_form_options(&self, _org: &str, repo: &str) -> Result<FormOptions> {
        let project = self.get(&format!("/project/{repo}"), &[]).await?;
        let repo_id = project
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape(format!("no project {repo}")))?
            .to_string();

        let issue_types: Vec<IdName> = project
            .get("issueTypes")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|t| !t.get("subtask").and_then(Value::as_bool).unwrap_or(false))
                    .filter_map(id_name)
                    .collect()
            })
            .unwrap_or_default();

        let mut labels = self.repo_labels(_org, repo).await?;
        let labels: Vec<IdName> = labels
            .drain(..)
            .map(|l| IdName {
                id: l.id,
                name: l.name,
            })
            .collect();

        let users = self
            .get(
                "/user/assignable/search",
                &[("project", repo.to_string()), ("maxResults", "100".into())],
            )
            .await
            .ok()
            .and_then(|d| d.as_array().cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|u| {
                        Some(IdName {
                            id: u.get("accountId")?.as_str()?.to_string(),
                            name: u
                                .get("displayName")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(FormOptions {
            repo_id,
            labels,
            users,
            milestones: Vec::new(),
            projects: Vec::new(),
            issue_types,
        })
    }

    /// Create a Jira issue. `repo_id` is the project id; `issue_type_id` is
    /// required. A synthetic priority label id in `label_ids` is peeled to the
    /// native priority field. Returns `(number, url)`.
    pub async fn create_issue(&self, p: &NewIssueParams) -> Result<(u64, String)> {
        let mut priority: Option<&str> = None;
        let mut labels: Vec<String> = Vec::new();
        for id in &p.label_ids {
            match synthetic_priority_id_to_name(id) {
                Some(name) => priority = Some(name),
                None => labels.push(id.clone()),
            }
        }

        let mut fields = json!({
            "project": { "id": p.repo_id },
            "summary": p.title,
            "description": text_to_adf(&p.body),
        });
        let type_id = p
            .issue_type_id
            .as_ref()
            .ok_or_else(|| ProviderError::Api("Jira requires an issue type".into()))?;
        fields["issuetype"] = json!({ "id": type_id });
        if let Some(account) = p.assignee_ids.first() {
            fields["assignee"] = json!({ "accountId": account });
        }
        if !labels.is_empty() {
            fields["labels"] = json!(labels);
        }
        if let Some(name) = priority {
            fields["priority"] = json!({ "name": name });
        }

        let data = self.post("/issue", json!({ "fields": fields })).await?;
        let key = data
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape("createIssue returned no key".into()))?;
        Ok((
            key_to_number(key),
            format!("{}/browse/{key}", self.base_url),
        ))
    }
}

/// Join Jira's error payload (`errorMessages` array and/or `errors` map) into a
/// readable one-liner, falling back to the raw body then the status code.
fn parse_jira_errors(text: &str, status: reqwest::StatusCode) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let mut parts: Vec<String> = Vec::new();
        if let Some(arr) = v.get("errorMessages").and_then(Value::as_array) {
            parts.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
        }
        if let Some(map) = v.get("errors").and_then(Value::as_object) {
            parts.extend(
                map.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| format!("{k}: {s}"))),
            );
        }
        if !parts.is_empty() {
            return parts.join("; ");
        }
    }
    if !text.trim().is_empty() {
        return text.trim().to_string();
    }
    format!("HTTP {}", status.as_u16())
}

fn id_name(v: &Value) -> Option<IdName> {
    Some(IdName {
        id: v.get("id")?.as_str()?.to_string(),
        name: v.get("name")?.as_str()?.to_string(),
    })
}

/// Map one Jira issue JSON object into the domain `Issue`.
fn map_issue(raw: &Value) -> Issue {
    let key = raw.get("key").and_then(Value::as_str).unwrap_or_default();
    let f = raw.get("fields").cloned().unwrap_or(Value::Null);

    let closed = f
        .pointer("/status/statusCategory/key")
        .and_then(Value::as_str)
        == Some("done");

    let mut labels: Vec<Label> = f
        .get("labels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|name| Label {
                    name: name.to_string(),
                    color: String::new(),
                })
                .collect()
        })
        .unwrap_or_default();

    // Fold Jira's native priority into a synthetic priority:* label.
    if let Some(value) = f
        .pointer("/priority/name")
        .and_then(Value::as_str)
        .and_then(priority_name_to_value)
    {
        labels.insert(
            0,
            Label {
                name: format!("priority:{value}"),
                color: String::new(),
            },
        );
    }

    let dt = |ptr: &str| {
        f.get(ptr)
            .and_then(Value::as_str)
            .and_then(super::parse_jira_dt)
    };

    Issue {
        id: key.to_string(),
        number: key_to_number(key),
        title: f
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        body: f.get("description").map(adf_to_text).unwrap_or_default(),
        state: if closed {
            IssueState::Closed
        } else {
            IssueState::Open
        },
        url: String::new(),
        author: f
            .pointer("/reporter/displayName")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        assignees: f
            .pointer("/assignee/displayName")
            .and_then(Value::as_str)
            .map(|n| vec![n.to_string()])
            .unwrap_or_default(),
        labels,
        comment_count: f
            .pointer("/comment/total")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        created_at: dt("created").unwrap_or_else(chrono::Utc::now),
        updated_at: dt("updated").unwrap_or_else(chrono::Utc::now),
        closed_at: dt("resolutiondate"),
    }
}

struct ProjectRef {
    key: String,
}

/// Thin adapter over the inherent methods — inherent methods win name
/// resolution inside this impl, so each call reaches the real implementation.
#[async_trait::async_trait]
impl crate::provider::IssueProvider for Client {
    async fn org_issues(&self, org: &str, include_closed: bool) -> Result<Vec<RepoIssues>> {
        self.org_issues(org, include_closed).await
    }
    async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        self.comments(issue_id).await
    }
    async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()> {
        self.add_comment(issue_id, body).await
    }
    async fn update_comment(&self, issue_id: &str, comment_id: &str, body: &str) -> Result<()> {
        self.update_comment(issue_id, comment_id, body).await
    }
    async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()> {
        self.set_state(issue_id, state).await
    }
    async fn update_title(&self, issue_id: &str, title: &str) -> Result<()> {
        self.update_title(issue_id, title).await
    }
    async fn update_body(&self, issue_id: &str, body: &str) -> Result<()> {
        self.update_body(issue_id, body).await
    }
    async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<()> {
        self.set_assignees(issue_id, logins).await
    }
    async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<()> {
        self.set_labels(issue_id, repo, org, names).await
    }
    async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>> {
        self.repo_labels(org, repo).await
    }
    async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions> {
        self.repo_form_options(org, repo).await
    }
    async fn create_issue(&self, p: &NewIssueParams) -> Result<(u64, String)> {
        self.create_issue(p).await
    }
    fn rate_limit(&self) -> Option<RateLimitData> {
        self.rate_limit()
    }
    // supports_pr_summary defaults to false — Jira has no GitHub PR links.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issue(extra: Value) -> Value {
        let mut base = json!({
            "key": "PROJ-42",
            "fields": {
                "summary": "Do the thing",
                "description": {
                    "type": "doc", "version": 1,
                    "content": [{ "type": "paragraph", "content": [
                        { "type": "text", "text": "body text" }
                    ]}]
                },
                "status": { "statusCategory": { "key": "indeterminate" } },
                "reporter": { "displayName": "Ada" },
                "assignee": { "displayName": "Grace" },
                "priority": { "name": "High" },
                "labels": ["backend", "urgent-ish"],
                "comment": { "total": 3 },
                "created": "2026-01-01T00:00:00.000+0000",
                "updated": "2026-01-02T00:00:00.000+0000",
                "resolutiondate": null
            }
        });
        merge(&mut base["fields"], extra);
        base
    }

    fn merge(target: &mut Value, extra: Value) {
        if let (Some(t), Value::Object(e)) = (target.as_object_mut(), extra) {
            for (k, v) in e {
                t.insert(k, v);
            }
        }
    }

    #[test]
    fn maps_open_issue_fields() {
        let issue = map_issue(&sample_issue(json!({})));
        assert_eq!(issue.id, "PROJ-42");
        assert_eq!(issue.number, 42);
        assert_eq!(issue.title, "Do the thing");
        assert_eq!(issue.body, "body text");
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.author, "Ada");
        assert_eq!(issue.assignees, vec!["Grace".to_string()]);
        assert_eq!(issue.comment_count, 3);
        // Native priority High → synthetic priority:high, inserted first.
        assert_eq!(issue.labels[0].name, "priority:high");
        assert_eq!(issue.priority_rank(), 3);
        assert!(issue.closed_at.is_none());
    }

    #[test]
    fn done_issue_is_closed_with_resolution_date() {
        let issue = map_issue(&sample_issue(json!({
            "status": { "statusCategory": { "key": "done" } },
            "resolutiondate": "2026-01-03T00:00:00.000+0000"
        })));
        assert_eq!(issue.state, IssueState::Closed);
        assert!(issue.closed_at.is_some());
    }

    #[test]
    fn absent_assignee_and_priority() {
        let issue = map_issue(&sample_issue(json!({
            "assignee": null,
            "priority": null,
            "labels": []
        })));
        assert!(issue.assignees.is_empty());
        assert!(issue.labels.is_empty());
        assert_eq!(issue.priority_rank(), 0);
    }

    #[test]
    fn error_payload_joins_messages() {
        let body = r#"{"errorMessages":["Boom"],"errors":{"summary":"required"}}"#;
        let msg = parse_jira_errors(body, reqwest::StatusCode::BAD_REQUEST);
        assert!(msg.contains("Boom"), "{msg}");
        assert!(msg.contains("summary: required"), "{msg}");
    }

    #[test]
    fn error_payload_falls_back_to_status() {
        let msg = parse_jira_errors("", reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(msg, "HTTP 500");
    }
}
