use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{priority_int_to_value, priority_value_to_int, synthetic_priority_id_to_int};
use crate::provider::error::{ProviderError, RATE_LIMIT_MSG_PREFIX, Result};
use crate::provider::types::{
    Comment, FormOptions, IdName, Issue, IssueState, Label, NewIssueParams, RateLimitData,
    RepoIssues, RepoLabel,
};

const API_URL: &str = "https://api.linear.app/graphql";
const TEAMS_PAGE: u32 = 50;
const ISSUES_PAGE: u32 = 100;

/// Async Linear GraphQL client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    rate_limit: Arc<Mutex<Option<RateLimitData>>>,
}

impl Client {
    pub fn new(key: String) -> anyhow::Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        // Personal API keys go in the Authorization header raw (no "Bearer").
        let mut auth = reqwest::header::HeaderValue::from_str(&key)?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .user_agent(concat!("gh-issues/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .build()?;
        Ok(Self {
            http,
            rate_limit: Arc::new(Mutex::new(None)),
        })
    }

    pub fn rate_limit(&self) -> Option<RateLimitData> {
        *self.rate_limit.lock().unwrap()
    }

    fn update_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
        };
        let remaining = get("x-ratelimit-requests-remaining");
        let limit = get("x-ratelimit-requests-limit");
        // Linear reports the reset as epoch milliseconds.
        let reset_ms = get("x-ratelimit-requests-reset");
        if let (Some(remaining), Some(limit), Some(reset_ms)) = (remaining, limit, reset_ms) {
            *self.rate_limit.lock().unwrap() = Some(RateLimitData {
                remaining: remaining.max(0) as u64,
                limit: limit.max(0) as u64,
                reset: reset_ms / 1000,
            });
        }
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let resp = self
            .http
            .post(API_URL)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?;

        self.update_rate_limit(resp.headers());

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let msg = match *self.rate_limit.lock().unwrap() {
                Some(ref d) => format!(
                    "{RATE_LIMIT_MSG_PREFIX} — {}/{} used, resets {}",
                    d.remaining,
                    d.limit,
                    d.reset_time()
                ),
                None => RATE_LIMIT_MSG_PREFIX.to_string(),
            };
            return Err(ProviderError::RateLimited(msg));
        }

        let body: Value = resp.error_for_status()?.json().await?;
        if let Some(errors) = body
            .get("errors")
            .filter(|e| !e.as_array().is_none_or(|a| a.is_empty()))
        {
            if errors_contain_ratelimit(errors) {
                return Err(ProviderError::RateLimited(format!(
                    "{RATE_LIMIT_MSG_PREFIX} (Linear) — {}",
                    join_error_messages(errors)
                )));
            }
            return Err(ProviderError::Api(join_error_messages(errors)));
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| ProviderError::Shape("missing data".into()))
    }

    /// Fetch every team as a group, each with its issues. `org` is ignored —
    /// the workspace is fixed by the API key. `include_closed` keeps
    /// completed/canceled issues; otherwise they are filtered server-side.
    pub async fn org_issues(&self, _org: &str, include_closed: bool) -> Result<Vec<RepoIssues>> {
        // An empty filter matches all issues; otherwise exclude done states.
        let filter = if include_closed {
            json!({})
        } else {
            json!({ "state": { "type": { "nin": ["completed", "canceled"] } } })
        };

        // Phase 1: list teams (cheap — no nested issues). Fetching teams and
        // their issues in one query blows Linear's fixed complexity budget
        // (teams × issues × per-issue fields), so issues are paged per team.
        let mut teams: Vec<TeamNode> = Vec::new();
        let mut team_cursor: Option<String> = None;
        loop {
            let data = self
                .graphql(
                    TEAMS_QUERY,
                    json!({ "teamsFirst": TEAMS_PAGE, "teamCursor": team_cursor }),
                )
                .await?;
            let conn: TeamsConn = parse_at(&data, &["teams"])?;
            teams.extend(conn.nodes);
            if !conn.page_info.has_next_page {
                break;
            }
            team_cursor = conn.page_info.end_cursor;
        }

        // Phase 2: page each team's issues.
        let team_issues_query = format!("{TEAM_ISSUES_QUERY}{ISSUE_FIELDS}");
        let mut out = Vec::new();
        for team in teams {
            let mut issues: Vec<Issue> = Vec::new();
            let mut issue_cursor: Option<String> = None;
            loop {
                let data = self
                    .graphql(
                        &team_issues_query,
                        json!({
                            "teamId": team.id,
                            "issuesFirst": ISSUES_PAGE,
                            "issueCursor": issue_cursor,
                            "filter": filter,
                        }),
                    )
                    .await?;
                let conn: IssuesConn = parse_at(&data, &["team", "issues"])?;
                issues.extend(conn.nodes.iter().map(IssueNode::to_issue));
                if !conn.page_info.has_next_page {
                    break;
                }
                issue_cursor = conn.page_info.end_cursor;
            }
            out.push(RepoIssues {
                repo: team.key,
                // Linear teams have no public URL field; groups open nothing.
                repo_url: String::new(),
                issues,
            });
        }
        Ok(out)
    }

    pub async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        let data = self
            .graphql(
                "query($id: String!) {
                   issue(id: $id) {
                     comments(first: 100) {
                       nodes { body createdAt user { displayName } }
                     }
                   }
                 }",
                json!({ "id": issue_id }),
            )
            .await?;
        let nodes = data
            .pointer("/issue/comments/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Shape("missing comments".into()))?;
        nodes
            .iter()
            .map(|n| {
                Ok(Comment {
                    author: n
                        .pointer("/user/displayName")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    created_at: serde_json::from_value(n["createdAt"].clone())
                        .map_err(|e| ProviderError::Shape(e.to_string()))?,
                    body: n["body"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect()
    }

    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()> {
        self.graphql(
            "mutation($id: String!, $body: String!) {
               commentCreate(input: {issueId: $id, body: $body}) { success }
             }",
            json!({ "id": issue_id, "body": body }),
        )
        .await
        .map(drop)
    }

    /// Move the issue to one of its team's workflow states of the appropriate
    /// type: a `completed` state to close, an `unstarted`/`backlog`/`started`
    /// state to reopen. Linear has no single "closed" flag — state is a
    /// per-team object — so the target team's states are resolved first.
    pub async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()> {
        let data = self
            .graphql(
                "query($id: String!) {
                   issue(id: $id) {
                     team { states(first: 50) { nodes { id type position } } }
                   }
                 }",
                json!({ "id": issue_id }),
            )
            .await?;
        let states: Vec<WorkflowStateNode> =
            parse_at(&data, &["issue", "team", "states", "nodes"])?;
        let want_closed = matches!(state, IssueState::Closed);
        // Lowest position wins within the wanted category — a stable, sensible
        // default target (e.g. the first "Todo" when reopening).
        let target = states
            .iter()
            .filter(|s| is_closed_type(&s.state_type) == want_closed)
            .min_by(|a, b| a.position.total_cmp(&b.position))
            .ok_or_else(|| {
                ProviderError::Shape(format!(
                    "team has no {} workflow state",
                    if want_closed { "completed" } else { "open" }
                ))
            })?;
        self.graphql(
            "mutation($id: String!, $stateId: String!) {
               issueUpdate(id: $id, input: {stateId: $stateId}) { success }
             }",
            json!({ "id": issue_id, "stateId": target.id }),
        )
        .await
        .map(drop)
    }

    pub async fn update_title(&self, issue_id: &str, title: &str) -> Result<()> {
        self.graphql(
            "mutation($id: String!, $title: String!) {
               issueUpdate(id: $id, input: {title: $title}) { success }
             }",
            json!({ "id": issue_id, "title": title }),
        )
        .await
        .map(drop)
    }

    /// Linear issues have a single assignee. Only the first login is used;
    /// an empty list unassigns.
    pub async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<()> {
        let assignee_id = match logins.first() {
            Some(login) => Some(self.resolve_user_id(login).await?),
            None => None,
        };
        self.graphql(
            "mutation($id: String!, $assigneeId: String) {
               issueUpdate(id: $id, input: {assigneeId: $assigneeId}) { success }
             }",
            json!({ "id": issue_id, "assigneeId": assignee_id }),
        )
        .await
        .map(drop)
    }

    async fn resolve_user_id(&self, login: &str) -> Result<String> {
        let data = self
            .graphql(
                "query($q: String!) {
                   users(filter: {or: [{displayName: {eq: $q}}, {email: {eq: $q}}]}, first: 1) {
                     nodes { id }
                   }
                 }",
                json!({ "q": login }),
            )
            .await?;
        data.pointer("/users/nodes/0/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ProviderError::Shape(format!("unknown user {login}")))
    }

    /// Replace the issue's labels. A `priority:*` name is peeled off and routed
    /// to Linear's native priority field; the rest are resolved to real team
    /// label ids. Passing no `priority:*` name clears the priority.
    pub async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<()> {
        let mut priority: u8 = 0;
        let mut real_names: Vec<&String> = Vec::new();
        for name in names {
            match crate::provider::types::priority_value(name) {
                Some(value) => priority = priority_value_to_int(value).unwrap_or(0),
                None => real_names.push(name),
            }
        }

        let all = self.real_repo_labels(org, repo).await?;
        let mut label_ids = Vec::new();
        for name in real_names {
            let label = all
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| ProviderError::Shape(format!("no label named {name} in {repo}")))?;
            label_ids.push(label.id.clone());
        }
        self.graphql(
            "mutation($id: String!, $labelIds: [String!]!, $priority: Int!) {
               issueUpdate(id: $id, input: {labelIds: $labelIds, priority: $priority}) { success }
             }",
            json!({ "id": issue_id, "labelIds": label_ids, "priority": priority }),
        )
        .await
        .map(drop)
    }

    /// Real Linear labels for a team, keyed by team key. Does NOT include the
    /// synthetic priority labels — used by the mutation paths that must only
    /// see genuine label ids.
    async fn real_repo_labels(&self, _org: &str, repo: &str) -> Result<Vec<RepoLabel>> {
        let data = self
            .graphql(
                "query($key: String!) {
                   teams(filter: {key: {eq: $key}}, first: 1) {
                     nodes { labels(first: 100) { nodes { id name } } }
                   }
                 }",
                json!({ "key": repo }),
            )
            .await?;
        let nodes = data
            .pointer("/teams/nodes/0/labels/nodes")
            .cloned()
            .ok_or_else(|| ProviderError::Shape(format!("no team {repo}")))?;
        serde_json::from_value(nodes).map_err(|e| ProviderError::Shape(e.to_string()))
    }

    /// Team labels plus the synthetic `priority:*` labels, so the priority
    /// picker (`p`) and label editor (`l`) have priority entries to show.
    pub async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>> {
        let mut labels = self.real_repo_labels(org, repo).await?;
        for (id, name) in super::synthetic_priority_labels() {
            labels.push(RepoLabel { id, name });
        }
        Ok(labels)
    }

    /// New-issue form options for a team: team id, real labels + synthetic
    /// priority labels, and assignable members. Linear has no milestones or
    /// issue types in the GitHub sense, so those stay empty; `projects` maps
    /// to the team's Linear projects.
    pub async fn repo_form_options(&self, _org: &str, repo: &str) -> Result<FormOptions> {
        let data = self
            .graphql(
                "query($key: String!) {
                   teams(filter: {key: {eq: $key}}, first: 1) {
                     nodes {
                       id
                       labels(first: 100) { nodes { id name } }
                       members(first: 100) { nodes { id displayName } }
                       projects(first: 50) { nodes { id name } }
                     }
                   }
                 }",
                json!({ "key": repo }),
            )
            .await?;
        let team = data
            .pointer("/teams/nodes/0")
            .filter(|v| !v.is_null())
            .ok_or_else(|| ProviderError::Shape(format!("no team {repo}")))?;
        let repo_id = team
            .pointer("/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape("missing team.id".into()))?
            .to_string();

        let mut labels: Vec<IdName> = from_nodes(team, "/labels/nodes")?;
        for (id, name) in super::synthetic_priority_labels() {
            labels.push(IdName { id, name });
        }
        let users: Vec<IdDisplayName> = from_nodes(team, "/members/nodes")?;
        let projects: Vec<IdName> = from_nodes(team, "/projects/nodes")?;

        Ok(FormOptions {
            repo_id,
            labels,
            users: users.into_iter().map(IdDisplayName::into_id_name).collect(),
            milestones: Vec::new(),
            projects,
            issue_types: Vec::new(),
        })
    }

    /// Create a Linear issue. `repo_id` is the team id. A synthetic priority
    /// label id in `label_ids` is peeled to the native priority field; a
    /// `project_id` sets the issue's Linear project. Returns `(number, url)`.
    pub async fn create_issue(&self, p: &NewIssueParams) -> Result<(u64, String)> {
        let mut priority: Option<u8> = None;
        let mut label_ids: Vec<String> = Vec::new();
        for id in &p.label_ids {
            match synthetic_priority_id_to_int(id) {
                Some(n) => priority = Some(n),
                None => label_ids.push(id.clone()),
            }
        }

        let mut input = json!({
            "teamId": p.repo_id,
            "title": p.title,
            "description": p.body,
        });
        if let Some(first) = p.assignee_ids.first() {
            input["assigneeId"] = json!(first);
        }
        if !label_ids.is_empty() {
            input["labelIds"] = json!(label_ids);
        }
        if let Some(n) = priority {
            input["priority"] = json!(n);
        }
        if let Some(project) = &p.project_id {
            input["projectId"] = json!(project);
        }
        let data = self
            .graphql(
                "mutation($input: IssueCreateInput!) {
                   issueCreate(input: $input) { issue { number url } }
                 }",
                json!({ "input": input }),
            )
            .await?;
        let issue = data
            .pointer("/issueCreate/issue")
            .filter(|v| !v.is_null())
            .ok_or_else(|| ProviderError::Shape("issueCreate returned no issue".into()))?;
        let number = issue.get("number").and_then(Value::as_u64).unwrap_or(0);
        let url = issue
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((number, url))
    }
}

fn errors_contain_ratelimit(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        arr.iter().any(|e| {
            e.pointer("/extensions/code")
                .and_then(Value::as_str)
                .is_some_and(|c| c.eq_ignore_ascii_case("RATELIMITED"))
        })
    })
}

fn join_error_messages(errors: &Value) -> String {
    errors
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| errors.to_string())
}

fn parse_at<T: for<'de> Deserialize<'de>>(data: &Value, path: &[&str]) -> Result<T> {
    let mut cur = data;
    for seg in path {
        cur = cur
            .get(seg)
            .ok_or_else(|| ProviderError::Shape(format!("missing {}", path.join("."))))?;
    }
    serde_json::from_value(cur.clone()).map_err(|e| ProviderError::Shape(e.to_string()))
}

fn from_nodes<T: for<'de> Deserialize<'de>>(v: &Value, ptr: &str) -> Result<T> {
    let nodes = v
        .pointer(ptr)
        .cloned()
        .ok_or_else(|| ProviderError::Shape(format!("missing {ptr}")))?;
    serde_json::from_value(nodes).map_err(|e| ProviderError::Shape(e.to_string()))
}

/// Linear workflow-state types that mean the issue is done/closed.
fn is_closed_type(state_type: &str) -> bool {
    matches!(state_type, "completed" | "canceled")
}

// ---- response DTOs ----

const ISSUE_FIELDS: &str = "fragment IssueFields on Issue {
  id number title description url priority createdAt updatedAt completedAt canceledAt
  state { type }
  creator { displayName }
  assignee { displayName }
  labels(first: 20) { nodes { id name } }
}";

const TEAMS_QUERY: &str = "query($teamsFirst: Int!, $teamCursor: String) {
  teams(first: $teamsFirst, after: $teamCursor) {
    nodes { id key name }
    pageInfo { hasNextPage endCursor }
  }
}
";

const TEAM_ISSUES_QUERY: &str =
    "query($teamId: String!, $issuesFirst: Int!, $issueCursor: String, $filter: IssueFilter) {
  team(id: $teamId) {
    issues(first: $issuesFirst, after: $issueCursor, includeArchived: false, filter: $filter) {
      nodes { ...IssueFields }
      pageInfo { hasNextPage endCursor }
    }
  }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TeamsConn {
    nodes: Vec<TeamNode>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
struct TeamNode {
    id: String,
    key: String,
    #[allow(dead_code)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct IssuesConn {
    nodes: Vec<IssueNode>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueNode {
    id: String,
    number: u64,
    title: String,
    #[serde(default)]
    description: Option<String>,
    url: String,
    #[serde(default)]
    priority: u8,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    canceled_at: Option<chrono::DateTime<chrono::Utc>>,
    state: StateRef,
    #[serde(default)]
    creator: Option<PersonRef>,
    #[serde(default)]
    assignee: Option<PersonRef>,
    labels: LabelsConn,
}

#[derive(Debug, Deserialize)]
struct StateRef {
    #[serde(rename = "type")]
    state_type: String,
}

#[derive(Debug, Deserialize)]
struct PersonRef {
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct LabelsConn {
    nodes: Vec<RepoLabel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowStateNode {
    id: String,
    #[serde(rename = "type")]
    state_type: String,
    position: f64,
}

#[derive(Debug, Deserialize)]
struct IdDisplayName {
    id: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

impl IdDisplayName {
    fn into_id_name(self) -> IdName {
        IdName {
            id: self.id,
            name: self.display_name,
        }
    }
}

impl IssueNode {
    fn to_issue(&self) -> Issue {
        let closed = is_closed_type(&self.state.state_type);
        let mut labels: Vec<Label> = self
            .labels
            .nodes
            .iter()
            .map(|l| Label {
                name: l.name.clone(),
                color: String::new(),
            })
            .collect();
        // Fold Linear's native priority into a synthetic priority:* label so
        // the app's sort/colour/filter machinery sees it like any other repo.
        if let Some(value) = priority_int_to_value(self.priority) {
            labels.insert(
                0,
                Label {
                    name: format!("priority:{value}"),
                    color: String::new(),
                },
            );
        }

        Issue {
            id: self.id.clone(),
            number: self.number,
            title: self.title.clone(),
            body: self.description.clone().unwrap_or_default(),
            state: if closed {
                IssueState::Closed
            } else {
                IssueState::Open
            },
            url: self.url.clone(),
            author: self
                .creator
                .as_ref()
                .map(|c| c.display_name.clone())
                .unwrap_or_else(|| "unknown".into()),
            assignees: self
                .assignee
                .as_ref()
                .map(|a| vec![a.display_name.clone()])
                .unwrap_or_default(),
            labels,
            comment_count: 0,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.completed_at.or(self.canceled_at),
        }
    }
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
    async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()> {
        self.set_state(issue_id, state).await
    }
    async fn update_title(&self, issue_id: &str, title: &str) -> Result<()> {
        self.update_title(issue_id, title).await
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
    // supports_pr_summary defaults to false — Linear has no GitHub PR links.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_node(v: Value) -> IssueNode {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn open_issue_maps_fields() {
        let n = issue_node(json!({
            "id": "iss_1", "number": 42, "title": "Do the thing",
            "description": "body text", "url": "https://linear.app/x/issue/ENG-42",
            "priority": 2,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
            "completedAt": null, "canceledAt": null,
            "state": {"type": "started"},
            "creator": {"displayName": "Ada"},
            "assignee": {"displayName": "Grace"},
            "labels": {"nodes": [{"id": "lab_1", "name": "bug"}]}
        }));
        let issue = n.to_issue();
        assert_eq!(issue.number, 42);
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.author, "Ada");
        assert_eq!(issue.assignees, vec!["Grace".to_string()]);
        // Native priority 2 → synthetic priority:high, inserted first.
        assert_eq!(issue.labels[0].name, "priority:high");
        assert_eq!(issue.labels[1].name, "bug");
        assert_eq!(issue.priority_rank(), 3); // high
        assert!(issue.closed_at.is_none());
    }

    #[test]
    fn completed_issue_is_closed_with_closed_at() {
        let n = issue_node(json!({
            "id": "iss_2", "number": 7, "title": "Done",
            "description": null, "url": "u", "priority": 0,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
            "completedAt": "2026-01-03T00:00:00Z", "canceledAt": null,
            "state": {"type": "completed"},
            "creator": null, "assignee": null,
            "labels": {"nodes": []}
        }));
        let issue = n.to_issue();
        assert_eq!(issue.state, IssueState::Closed);
        assert_eq!(issue.author, "unknown");
        assert!(issue.assignees.is_empty());
        // Priority 0 → no synthetic label.
        assert!(issue.labels.is_empty());
        assert!(issue.closed_at.is_some());
    }

    #[test]
    fn canceled_issue_is_closed() {
        let n = issue_node(json!({
            "id": "iss_3", "number": 8, "title": "Nope",
            "description": null, "url": "u", "priority": 1,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
            "completedAt": null, "canceledAt": "2026-01-04T00:00:00Z",
            "state": {"type": "canceled"},
            "creator": {"displayName": "X"}, "assignee": null,
            "labels": {"nodes": []}
        }));
        let issue = n.to_issue();
        assert_eq!(issue.state, IssueState::Closed);
        assert_eq!(issue.labels[0].name, "priority:urgent");
        assert_eq!(
            issue.closed_at.unwrap().to_rfc3339(),
            "2026-01-04T00:00:00+00:00"
        );
    }

    #[test]
    fn is_closed_type_covers_done_states() {
        assert!(is_closed_type("completed"));
        assert!(is_closed_type("canceled"));
        assert!(!is_closed_type("started"));
        assert!(!is_closed_type("backlog"));
        assert!(!is_closed_type("unstarted"));
    }

    #[test]
    fn ratelimit_error_detected_by_extension_code() {
        let errors = json!([{"message": "slow down", "extensions": {"code": "RATELIMITED"}}]);
        assert!(errors_contain_ratelimit(&errors));
        let other = json!([{"message": "nope", "extensions": {"code": "AUTHENTICATION"}}]);
        assert!(!errors_contain_ratelimit(&other));
    }

    #[test]
    fn join_error_messages_reads_message_fields() {
        let errors = json!([{"message": "first"}, {"message": "second"}]);
        assert_eq!(join_error_messages(&errors), "first; second");
    }
}
