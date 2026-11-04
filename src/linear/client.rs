use serde::Deserialize;
use serde_json::{Value, json};

use super::{priority_int_to_value, priority_value_to_int, synthetic_priority_id_to_int};
use crate::provider::error::{ProviderError, Result, found};
use crate::provider::http::{
    RateLimitStore, build_http_client, graphql_body, graphql_data, header_i64, join_error_messages,
    parse_at,
};
use crate::provider::scope::FetchScope;
use crate::provider::types::{
    Comment, FormOptions, IdName, Issue, IssueState, IssueStub, Label, NewIssueParams,
    RateLimitData, RepoIssues, RepoLabel,
};

const API_URL: &str = "https://api.linear.app/graphql";
const TEAMS_PAGE: u32 = 50;
const ISSUES_PAGE: u32 = 100;

/// Async Linear GraphQL client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    rate_limit: RateLimitStore,
}

impl Client {
    pub fn new(key: String) -> anyhow::Result<Self> {
        // Personal API keys go in the Authorization header raw (no "Bearer").
        Ok(Self {
            http: build_http_client(&key, &[])?,
            rate_limit: RateLimitStore::default(),
        })
    }

    pub fn rate_limit(&self) -> Option<RateLimitData> {
        self.rate_limit.get()
    }

    /// Linear reports the reset as epoch **milliseconds**, so it is divided
    /// down to the seconds `RateLimitData` stores.
    fn update_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        let remaining = header_i64(headers, "x-ratelimit-requests-remaining");
        let limit = header_i64(headers, "x-ratelimit-requests-limit");
        let reset_ms = header_i64(headers, "x-ratelimit-requests-reset");
        if let (Some(remaining), Some(limit), Some(reset_ms)) = (remaining, limit, reset_ms) {
            self.rate_limit.set(RateLimitData {
                remaining: remaining.max(0) as u64,
                limit: limit.max(0) as u64,
                reset: reset_ms / 1000,
                last_cost: None,
            });
        }
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let resp = self
            .http
            .post(API_URL)
            .json(&graphql_body(query, variables))
            .send()
            .await?;

        self.update_rate_limit(resp.headers());

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited(self.rate_limit.message(None)));
        }

        let body: Value = resp.error_for_status()?.json().await?;
        graphql_data(&body, |errors| {
            if errors_contain_ratelimit(errors) {
                return Some(ProviderError::RateLimited(format!(
                    "{} (Linear) — {}",
                    crate::provider::error::RATE_LIMIT_MSG_PREFIX,
                    join_error_messages(errors)
                )));
            }
            errors_all_not_found(errors)
                .then(|| ProviderError::NotFound(join_error_messages(errors)))
        })
    }

    /// One issue by id (#229); `Ok(None)` once Linear no longer finds it.
    pub async fn issue(&self, issue_id: &str) -> Result<Option<Issue>> {
        let query = format!("{ISSUE_QUERY}{ISSUE_FIELDS}");
        let Some(data) = found(self.graphql(&query, json!({ "id": issue_id })).await)? else {
            return Ok(None);
        };
        let node: Option<IssueNode> = parse_at(&data, &["issue"])?;
        Ok(node.map(|n| n.to_issue()))
    }

    /// One team's issues by team key — the group name `org_issues` reports
    /// (#229). `Ok(None)` when no team has that key any more.
    pub async fn repo_issues(
        &self,
        _org: &str,
        team_key: &str,
        include_closed: bool,
    ) -> Result<Option<RepoIssues>> {
        let data = self
            .graphql(TEAM_BY_KEY_QUERY, json!({ "key": team_key }))
            .await?;
        let conn: TeamsConn = parse_at(&data, &["teams"])?;
        let Some(team) = conn.nodes.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(self.team_issues(team, include_closed).await?))
    }

    /// Page one team's issues into its group.
    async fn team_issues(&self, team: TeamNode, include_closed: bool) -> Result<RepoIssues> {
        let filter = issue_filter(include_closed);
        let team_issues_query = format!("{TEAM_ISSUES_QUERY}{ISSUE_FIELDS}");
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
        Ok(RepoIssues {
            repo: team.key,
            // Linear teams have no public URL field; groups open nothing.
            repo_url: String::new(),
            issues,
        })
    }

    /// Fetch every team as a group, each with its issues. `org` is ignored —
    /// the workspace is fixed by the API key. `include_closed` keeps
    /// completed/canceled issues; otherwise they are filtered server-side.
    pub async fn org_issues(&self, _org: &str, include_closed: bool) -> Result<Vec<RepoIssues>> {
        // Phase 1: list teams (cheap — no nested issues). Fetching teams and
        // their issues in one query blows Linear's fixed complexity budget
        // (teams × issues × per-issue fields), so issues are paged per team.
        let teams = self.teams().await?;

        // Phase 2: page each team's issues.
        let mut out = Vec::new();
        for team in teams {
            out.push(self.team_issues(team, include_closed).await?);
        }
        Ok(out)
    }

    /// Every team key: the group names `org_issues` reports (#230).
    pub async fn repo_names(&self, _org: &str) -> Result<Vec<String>> {
        Ok(self.teams().await?.into_iter().map(|t| t.key).collect())
    }

    /// Every team in the workspace, paged.
    async fn teams(&self) -> Result<Vec<TeamNode>> {
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
        Ok(teams)
    }

    pub async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        let data = self
            .graphql(
                "query($id: String!) {
                   issue(id: $id) {
                     comments(first: 100) {
                       nodes { id body createdAt user { displayName } }
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
                    id: n["id"].as_str().unwrap_or_default().to_string(),
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

    pub async fn update_comment(&self, comment_id: &str, body: &str) -> Result<()> {
        self.graphql(
            "mutation($id: String!, $body: String!) {
               commentUpdate(id: $id, input: {body: $body}) { success }
             }",
            json!({ "id": comment_id, "body": body }),
        )
        .await
        .map(drop)
    }

    /// Edit an issue's description. Linear calls the body `description`.
    pub async fn update_body(&self, issue_id: &str, body: &str) -> Result<Issue> {
        self.issue_update(issue_id, json!({ "description": body }))
            .await
    }

    /// Run `issueUpdate` with `input` and return the issue it reports, mapped
    /// by the same fragment the bulk fetch uses (#196).
    async fn issue_update(&self, issue_id: &str, input: Value) -> Result<Issue> {
        let query = format!("{ISSUE_UPDATE_MUTATION}{ISSUE_FIELDS}");
        let data = self
            .graphql(&query, json!({ "id": issue_id, "input": input }))
            .await?;
        issue_from_payload(&data)
    }

    /// Move the issue to one of its team's workflow states of the appropriate
    /// type: a `completed` state to close, an `unstarted`/`backlog`/`started`
    /// state to reopen. Linear has no single "closed" flag — state is a
    /// per-team object — so the target team's states are resolved first.
    pub async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<Issue> {
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
        self.issue_update(issue_id, json!({ "stateId": target.id }))
            .await
    }

    pub async fn update_title(&self, issue_id: &str, title: &str) -> Result<Issue> {
        self.issue_update(issue_id, json!({ "title": title })).await
    }

    /// Linear issues have a single assignee. Only the first login is used;
    /// an empty list unassigns.
    pub async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<Issue> {
        let assignee_id = match logins.first() {
            Some(login) => Some(self.resolve_user_id(login).await?),
            None => None,
        };
        self.issue_update(issue_id, json!({ "assigneeId": assignee_id }))
            .await
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
    ) -> Result<Issue> {
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
        self.issue_update(
            issue_id,
            json!({ "labelIds": label_ids, "priority": priority }),
        )
        .await
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

    /// Move the issue to another team ("repo" in this app's terminology).
    /// Linear remaps the issue's workflow state and label set to the target
    /// team server-side — this app makes no attempt to reconcile them itself.
    /// Returns the moved issue (#226) so the list patches it in place.
    pub async fn move_issue(&self, issue_id: &str, _org: &str, target: &str) -> Result<Issue> {
        let data = self
            .graphql(
                "query($key: String!) {
                   teams(filter: {key: {eq: $key}}, first: 1) {
                     nodes { id }
                   }
                 }",
                json!({ "key": target }),
            )
            .await?;
        let team_id = data
            .pointer("/teams/nodes/0/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape(format!("no team {target}")))?
            .to_string();
        self.issue_update(issue_id, json!({ "teamId": team_id }))
            .await
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
    /// `project_id` sets the issue's Linear project. Returns the created issue (#249).
    pub async fn create_issue(&self, p: &NewIssueParams) -> Result<Issue> {
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
        let query = format!("{ISSUE_CREATE_MUTATION}{ISSUE_FIELDS}");
        let data = self.graphql(&query, json!({ "input": input })).await?;
        issue_at(&data, "issueCreate")
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
  parent { id number title url state { type } team { key } }
}";

/// `issueUpdate` reporting the updated issue; append [`ISSUE_FIELDS`].
const ISSUE_UPDATE_MUTATION: &str = "mutation($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) { success issue { ...IssueFields } }
}
";

/// `issueCreate` reporting the new issue (#249); append [`ISSUE_FIELDS`].
const ISSUE_CREATE_MUTATION: &str = "mutation($input: IssueCreateInput!) {
  issueCreate(input: $input) { success issue { ...IssueFields } }
}
";

/// Parse `data.issueUpdate.issue` into an [`Issue`].
fn issue_from_payload(data: &Value) -> Result<Issue> {
    issue_at(data, "issueUpdate")
}

/// Parse `data.<field>.issue` into an [`Issue`].
fn issue_at(data: &Value, field: &str) -> Result<Issue> {
    let node: Option<IssueNode> = parse_at(data, &[field, "issue"])?;
    node.map(|n| n.to_issue())
        .ok_or_else(|| ProviderError::Shape(format!("{field} returned no issue")))
}

const TEAMS_QUERY: &str = "query($teamsFirst: Int!, $teamCursor: String) {
  teams(first: $teamsFirst, after: $teamCursor) {
    nodes { id key }
    pageInfo { hasNextPage endCursor }
  }
}
";

/// One team by key (#229). Keys are unique per workspace, so at most one node.
const TEAM_BY_KEY_QUERY: &str = "query($key: String!) {
  teams(first: 1, filter: { key: { eq: $key } }) {
    nodes { id key }
    pageInfo { hasNextPage endCursor }
  }
}
";

/// One issue by id; append [`ISSUE_FIELDS`].
const ISSUE_QUERY: &str = "query($id: String!) {
  issue(id: $id) { ...IssueFields }
}
";

/// An empty filter matches all issues; otherwise exclude done states.
fn issue_filter(include_closed: bool) -> Value {
    if include_closed {
        json!({})
    } else {
        json!({ "state": { "type": { "nin": ["completed", "canceled"] } } })
    }
}

/// True when every error says the entity is missing — Linear answers an
/// unknown id with `Entity not found: <Type>` (#229). Matched on the message,
/// because Linear files these under a generic input-error code.
fn errors_all_not_found(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        !arr.is_empty()
            && arr.iter().all(|e| {
                e.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.starts_with("Entity not found"))
            })
    })
}

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
    /// The parent issue, one level only (#234): a deeper chain would add to
    /// Linear's per-query complexity on every issue of every page.
    #[serde(default)]
    parent: Option<ParentRef>,
}

#[derive(Debug, Deserialize)]
struct ParentRef {
    id: String,
    number: u64,
    title: String,
    url: String,
    state: StateRef,
    team: TeamKeyRef,
}

#[derive(Debug, Deserialize)]
struct TeamKeyRef {
    key: String,
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
                rank: None,
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
                    rank: None,
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
            ancestors: self
                .parent
                .iter()
                .map(|p| IssueStub {
                    id: p.id.clone(),
                    number: p.number,
                    title: p.title.clone(),
                    state: if is_closed_type(&p.state.state_type) {
                        IssueState::Closed
                    } else {
                        IssueState::Open
                    },
                    url: p.url.clone(),
                    repo: p.team.key.clone(),
                    owner: String::new(),
                })
                .collect(),
            // Linear reports children only as a connection, which would
            // count against its complexity budget on every issue (#234).
            sub_issues: None,
            relations: None,
        }
    }
}

/// Thin adapter over the inherent methods — inherent methods win name
/// resolution inside this impl, so each call reaches the real implementation.
#[async_trait::async_trait]
impl crate::provider::IssueProvider for Client {
    // Linear honours only the state part of a `FetchScope` (#230); the
    // client-side filters cover the rest.
    async fn org_issues(&self, org: &str, scope: &FetchScope) -> Result<Vec<RepoIssues>> {
        self.org_issues(org, scope.include_closed).await
    }
    async fn issue(&self, issue_id: &str) -> Result<Option<Issue>> {
        self.issue(issue_id).await
    }
    async fn repo_issues(
        &self,
        org: &str,
        repo: &str,
        scope: &FetchScope,
    ) -> Result<Option<RepoIssues>> {
        self.repo_issues(org, repo, scope.include_closed).await
    }
    async fn repo_names(&self, org: &str) -> Result<Vec<String>> {
        self.repo_names(org).await
    }
    async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        self.comments(issue_id).await
    }
    async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()> {
        self.add_comment(issue_id, body).await
    }
    async fn update_comment(&self, _issue_id: &str, comment_id: &str, body: &str) -> Result<()> {
        self.update_comment(comment_id, body).await
    }
    async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<Issue> {
        self.set_state(issue_id, state).await
    }
    async fn update_title(&self, issue_id: &str, title: &str) -> Result<Issue> {
        self.update_title(issue_id, title).await
    }
    async fn update_body(&self, issue_id: &str, body: &str) -> Result<Issue> {
        self.update_body(issue_id, body).await
    }
    async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<Issue> {
        self.set_assignees(issue_id, logins).await
    }
    async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<Issue> {
        self.set_labels(issue_id, repo, org, names).await
    }
    async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>> {
        self.repo_labels(org, repo).await
    }
    async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions> {
        self.repo_form_options(org, repo).await
    }
    async fn create_issue(&self, p: &NewIssueParams) -> Result<Issue> {
        self.create_issue(p).await
    }
    fn rate_limit(&self) -> Option<RateLimitData> {
        self.rate_limit()
    }
    fn host(&self) -> String {
        "linear.app".into()
    }
    // supports_pr_summary defaults to false — Linear has no GitHub PR links.
    fn supports_move(&self) -> bool {
        true
    }
    async fn move_issue(&self, issue_id: &str, org: &str, target: &str) -> Result<Issue> {
        self.move_issue(issue_id, org, target).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_node(v: Value) -> IssueNode {
        serde_json::from_value(v).unwrap()
    }

    /// #229: Linear's answer for an unknown id. Not yet seen live (no
    /// workspace to test against); shape from Linear's API docs.
    #[test]
    fn entity_not_found_errors_are_not_found() {
        let gone = json!([{"message": "Entity not found: Issue",
            "extensions": {"code": "INPUT_ERROR"}}]);
        assert!(errors_all_not_found(&gone));
        let other = json!([{"message": "Argument Validation Error"}]);
        assert!(!errors_all_not_found(&other));
        assert!(!errors_all_not_found(&json!([])));
    }

    #[test]
    fn closed_filter_matches_org_issues() {
        assert_eq!(issue_filter(true), json!({}));
        assert_eq!(
            issue_filter(false),
            json!({ "state": { "type": { "nin": ["completed", "canceled"] } } })
        );
    }

    #[test]
    fn team_lookup_is_by_key() {
        assert!(TEAM_BY_KEY_QUERY.contains("key: { eq: $key }"));
        assert!(ISSUE_QUERY.contains("...IssueFields"));
    }

    /// #231: teams are grouped and looked up by key; the name was fetched
    /// and never read.
    #[test]
    fn team_queries_select_only_what_is_read() {
        for query in [TEAMS_QUERY, TEAM_BY_KEY_QUERY] {
            assert!(query.contains("nodes { id key }"), "{query}");
        }
    }

    /// #234: a Linear parent becomes a one-level ancestry, grouped by team key.
    #[test]
    fn a_parent_maps_to_a_one_level_ancestry() {
        assert!(
            ISSUE_FIELDS.contains("parent { id number title url state { type } team { key } }")
        );
        let n = issue_node(json!({
            "id": "iss_2", "number": 7, "title": "child", "url": "u",
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
            "state": {"type": "started"}, "labels": {"nodes": []},
            "parent": {"id": "iss_1", "number": 3, "title": "root", "url": "pu",
                       "state": {"type": "completed"}, "team": {"key": "ENG"}}
        }));
        let issue = n.to_issue();
        assert_eq!(issue.ancestors.len(), 1);
        let p = &issue.ancestors[0];
        assert_eq!((p.number, p.repo.as_str()), (3, "ENG"));
        assert_eq!(p.state, IssueState::Closed);
        assert!(issue.sub_issues.is_none() && issue.relations.is_none());
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

    /// #196: the mutation payload carries the issue through the bulk
    /// fetch's fragment, so a patch maps exactly like a fetched issue.
    #[test]
    fn issue_update_payload_maps_to_the_updated_issue() {
        let data = json!({ "issueUpdate": { "success": true, "issue": {
            "id": "iss_1", "number": 42, "title": "Do the thing",
            "description": "body", "url": "u", "priority": 0,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-05T00:00:00Z",
            "completedAt": "2026-01-05T00:00:00Z", "canceledAt": null,
            "state": {"type": "completed"},
            "creator": {"displayName": "Ada"}, "assignee": null,
            "labels": {"nodes": [{"id": "lab_2", "name": "wontfix"}]}
        }}});
        let issue = issue_from_payload(&data).unwrap();
        assert_eq!(issue.id, "iss_1");
        assert_eq!(issue.state, IssueState::Closed);
        assert_eq!(issue.labels[0].name, "wontfix");
    }

    #[test]
    fn issue_create_payload_maps_to_the_created_issue() {
        let data = json!({ "issueCreate": { "success": true, "issue": {
            "id": "iss_9", "number": 9, "title": "New",
            "description": "", "url": "u", "priority": 0,
            "createdAt": "2026-11-04T00:00:00Z", "updatedAt": "2026-11-04T00:00:00Z",
            "completedAt": null, "canceledAt": null,
            "state": {"type": "unstarted"},
            "creator": {"displayName": "Ada"}, "assignee": null,
            "labels": {"nodes": []}
        }}});
        let issue = issue_at(&data, "issueCreate").unwrap();
        assert_eq!(issue.id, "iss_9");
        assert_eq!(issue.number, 9);
        assert_eq!(issue.state, IssueState::Open);
    }

    #[test]
    fn issue_create_mutation_selects_the_shared_fragment() {
        assert!(ISSUE_CREATE_MUTATION.contains("issue { ...IssueFields }"));
        assert!(ISSUE_CREATE_MUTATION.contains("$input: IssueCreateInput!"));
    }

    #[test]
    fn issue_update_payload_without_an_issue_is_a_shape_error() {
        let data = json!({ "issueUpdate": { "success": true, "issue": null } });
        assert!(matches!(
            issue_from_payload(&data),
            Err(ProviderError::Shape(_))
        ));
    }

    #[test]
    fn issue_update_mutation_selects_the_shared_fragment() {
        assert!(ISSUE_UPDATE_MUTATION.contains("issue { ...IssueFields }"));
        assert!(ISSUE_UPDATE_MUTATION.contains("$input: IssueUpdateInput!"));
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

    // ---- Characterisation goldens (issue #87) ---------------------------
    //
    // Linear reports its reset as epoch *milliseconds* while the shared
    // `RateLimitData` stores seconds. That conversion is easy to lose when
    // the rate-limit store moves to the provider layer, and had no test.

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn rate_limit_reset_converts_milliseconds_to_seconds() {
        let c = Client::new("k".into()).unwrap();
        c.update_rate_limit(&headers(&[
            ("x-ratelimit-requests-remaining", "900"),
            ("x-ratelimit-requests-limit", "1500"),
            ("x-ratelimit-requests-reset", "1800000000123"),
        ]));

        let rl = c.rate_limit().expect("rate limit recorded");
        assert_eq!(rl.remaining, 900);
        assert_eq!(rl.limit, 1500);
        assert_eq!(rl.reset, 1_800_000_000, "millis must be divided by 1000");
    }

    #[test]
    fn rate_limit_clamps_negative_counts_to_zero() {
        let c = Client::new("k".into()).unwrap();
        c.update_rate_limit(&headers(&[
            ("x-ratelimit-requests-remaining", "-5"),
            ("x-ratelimit-requests-limit", "1500"),
            ("x-ratelimit-requests-reset", "1800000000000"),
        ]));
        assert_eq!(c.rate_limit().unwrap().remaining, 0);
    }

    #[test]
    fn rate_limit_ignores_partial_header_sets() {
        let c = Client::new("k".into()).unwrap();
        c.update_rate_limit(&headers(&[("x-ratelimit-requests-remaining", "900")]));
        assert!(c.rate_limit().is_none());
    }
}
