use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::error::{GithubError, RATE_LIMIT_MSG_PREFIX, Result};
use super::types::{
    CheckContextInfo, CheckRollup, Comment, FormOptions, IdName, Issue, IssueState, Label,
    NewIssueParams, PrRef, PrState, PrSummary, RateLimitData, RepoIssues, RepoLabel,
    ReviewDecision, ReviewSummary, WorkflowRunInfo,
};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const REPOS_PAGE: u32 = 50;
const ISSUES_PAGE: u32 = 100;

/// Async GitHub GraphQL v4 client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    rate_limit: Arc<Mutex<Option<RateLimitData>>>,
}

impl Client {
    pub fn new(token: String) -> anyhow::Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?;
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

    /// Returns the most recently observed rate limit state.
    pub fn rate_limit(&self) -> Option<RateLimitData> {
        *self.rate_limit.lock().unwrap()
    }

    fn update_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        let remaining = headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let limit = headers
            .get("x-ratelimit-limit")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let (Some(remaining), Some(limit), Some(reset)) = (remaining, limit, reset) {
            *self.rate_limit.lock().unwrap() = Some(RateLimitData {
                remaining,
                limit,
                reset,
            });
        }
    }

    fn rate_limit_message(&self, data: &RateLimitData) -> String {
        format!(
            "{RATE_LIMIT_MSG_PREFIX} — {}/{} used, resets {}",
            data.remaining,
            data.limit,
            data.reset_time()
        )
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let resp = self
            .http
            .post(GRAPHQL_URL)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?;

        self.update_rate_limit(resp.headers());

        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            let rl = self.rate_limit.lock().unwrap();
            if let Some(ref data) = *rl
                && data.remaining == 0
            {
                return Err(GithubError::RateLimited(self.rate_limit_message(data)));
            }
        }

        let body: Value = resp.error_for_status()?.json().await?;
        if let Some(errors) = body
            .get("errors")
            .filter(|e| !e.as_array().is_none_or(|a| a.is_empty()))
        {
            // The GraphQL API reports the primary rate limit as an HTTP 200
            // with a RATE_LIMITED error entry, not as a 403.
            if errors_contain_rate_limited(errors) {
                let rl = self.rate_limit.lock().unwrap();
                let msg = match *rl {
                    Some(ref data) => self.rate_limit_message(data),
                    None => format!("{RATE_LIMIT_MSG_PREFIX} (GraphQL)"),
                };
                return Err(GithubError::RateLimited(msg));
            }
            return Err(GithubError::GraphQl(errors.to_string()));
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| GithubError::Shape("missing data".into()))
    }

    /// Fetch all issues for every repository owned by `org` — an
    /// organisation or a user account (`repositoryOwner` covers both).
    ///
    /// Iterates repositories with cursor pagination (avoids the 1000-result
    /// search API cap) and follows per-repo issue pagination where needed.
    pub async fn org_issues(&self, org: &str, include_closed: bool) -> Result<Vec<RepoIssues>> {
        let states = if include_closed {
            vec!["OPEN", "CLOSED"]
        } else {
            vec!["OPEN"]
        };
        let mut out = Vec::new();
        let mut repo_cursor: Option<String> = None;

        loop {
            let data = self
                .graphql(
                    ORG_ISSUES_QUERY,
                    json!({
                        "org": org,
                        "reposFirst": REPOS_PAGE,
                        "issuesFirst": ISSUES_PAGE,
                        "repoCursor": repo_cursor,
                        "states": states,
                    }),
                )
                .await?;

            // An unknown login yields `"repositoryOwner": null`, not an error.
            if data.get("repositoryOwner").is_some_and(Value::is_null) {
                return Err(GithubError::Shape(format!("no such org or user: {org}")));
            }
            let repos: RepositoriesConn = parse_at(&data, &["repositoryOwner", "repositories"])?;
            for repo in repos.nodes {
                // Repos with issues disabled can never hold issues — skip.
                if !repo.has_issues_enabled {
                    continue;
                }
                let mut issues: Vec<Issue> =
                    repo.issues.nodes.iter().map(IssueNode::to_issue).collect();

                // Follow issue pagination for repos with >ISSUES_PAGE issues.
                let mut page = repo.issues.page_info;
                while page.has_next_page {
                    let data = self
                        .graphql(
                            REPO_ISSUES_QUERY,
                            json!({
                                "owner": org,
                                "name": repo.name,
                                "issuesFirst": ISSUES_PAGE,
                                "issueCursor": page.end_cursor,
                                "states": states,
                            }),
                        )
                        .await?;
                    let conn: IssuesConn = parse_at(&data, &["repository", "issues"])?;
                    issues.extend(conn.nodes.iter().map(IssueNode::to_issue));
                    page = conn.page_info;
                }

                // Empty repos are kept: the hide-empty-repos filter decides
                // their visibility client-side, so toggling needs no refetch.
                out.push(RepoIssues {
                    repo: repo.name,
                    repo_url: repo.url,
                    issues,
                });
            }

            if !repos.page_info.has_next_page {
                break;
            }
            repo_cursor = repos.page_info.end_cursor;
        }
        Ok(out)
    }

    /// Fetch the comment thread of an issue by node id.
    pub async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        let data = self
            .graphql(COMMENTS_QUERY, json!({ "id": issue_id }))
            .await?;
        let nodes = data
            .pointer("/node/comments/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| GithubError::Shape("missing comments".into()))?;
        nodes
            .iter()
            .map(|n| {
                Ok(Comment {
                    author: login_at(n, "/author/login"),
                    created_at: serde_json::from_value(n["createdAt"].clone())
                        .map_err(|e| GithubError::Shape(e.to_string()))?,
                    body: n["body"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect()
    }

    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()> {
        self.graphql(
            "mutation($id: ID!, $body: String!) {
               addComment(input: {subjectId: $id, body: $body}) { clientMutationId }
             }",
            json!({ "id": issue_id, "body": body }),
        )
        .await
        .map(drop)
    }

    pub async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()> {
        let q = match state {
            IssueState::Closed => {
                "mutation($id: ID!) { closeIssue(input: {issueId: $id}) { clientMutationId } }"
            }
            IssueState::Open => {
                "mutation($id: ID!) { reopenIssue(input: {issueId: $id}) { clientMutationId } }"
            }
        };
        self.graphql(q, json!({ "id": issue_id })).await.map(drop)
    }

    pub async fn update_title(&self, issue_id: &str, title: &str) -> Result<()> {
        self.graphql(
            "mutation($id: ID!, $title: String!) {
               updateIssue(input: {id: $id, title: $title}) { clientMutationId }
             }",
            json!({ "id": issue_id, "title": title }),
        )
        .await
        .map(drop)
    }

    /// Replace the full assignee set. Logins are resolved to user ids.
    pub async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<()> {
        let mut ids = Vec::with_capacity(logins.len());
        for login in logins {
            let data = self
                .graphql(
                    "query($login: String!) { user(login: $login) { id } }",
                    json!({ "login": login }),
                )
                .await?;
            let id = data
                .pointer("/user/id")
                .and_then(Value::as_str)
                .ok_or_else(|| GithubError::Shape(format!("unknown user {login}")))?
                .to_string();
            ids.push(id);
        }
        self.graphql(
            "mutation($id: ID!, $assigneeIds: [ID!]) {
               updateIssue(input: {id: $id, assigneeIds: $assigneeIds}) { clientMutationId }
             }",
            json!({ "id": issue_id, "assigneeIds": ids }),
        )
        .await
        .map(drop)
    }

    /// Replace the full label set with labels named in `names` (must exist on the repo).
    pub async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<()> {
        let all = self.repo_labels(org, repo).await?;
        let mut ids = Vec::new();
        for name in names {
            let label = all
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| GithubError::Shape(format!("no label named {name} in {repo}")))?;
            ids.push(label.id.clone());
        }
        self.graphql(
            "mutation($id: ID!, $labelIds: [ID!]) {
               updateIssue(input: {id: $id, labelIds: $labelIds}) { clientMutationId }
             }",
            json!({ "id": issue_id, "labelIds": ids }),
        )
        .await
        .map(drop)
    }

    /// Everything the new-issue form's pickers need, in one query (plus a
    /// separate, failure-tolerant issue-types query — that field is an org
    /// feature not available for every owner and must not kill the form).
    pub async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) {
                     id
                     labels(first: 100, orderBy: {field: NAME, direction: ASC}) { nodes { id name } }
                     assignableUsers(first: 100) { nodes { id login } }
                     milestones(first: 50, states: [OPEN], orderBy: {field: DUE_DATE, direction: ASC}) { nodes { id title } }
                     projectsV2(first: 50) { nodes { id title } }
                   }
                 }",
                json!({ "owner": org, "name": repo }),
            )
            .await?;
        if data.get("repository").is_some_and(Value::is_null) {
            return Err(GithubError::Shape(format!("no repository {org}/{repo}")));
        }
        let repo_id = data
            .pointer("/repository/id")
            .and_then(Value::as_str)
            .ok_or_else(|| GithubError::Shape("missing repository.id".into()))?
            .to_string();

        let labels: Vec<IdName> = parse_at(&data, &["repository", "labels", "nodes"])?;
        let users: Vec<IdLogin> = parse_at(&data, &["repository", "assignableUsers", "nodes"])?;
        let milestones: Vec<IdTitle> = parse_at(&data, &["repository", "milestones", "nodes"])?;
        let projects: Vec<IdTitle> = parse_at(&data, &["repository", "projectsV2", "nodes"])?;

        let issue_types = self.issue_types(org, repo).await.unwrap_or_default();

        Ok(FormOptions {
            repo_id,
            labels,
            users: users.into_iter().map(IdLogin::into_id_name).collect(),
            milestones: milestones.into_iter().map(IdTitle::into_id_name).collect(),
            projects: projects.into_iter().map(IdTitle::into_id_name).collect(),
            issue_types,
        })
    }

    async fn issue_types(&self, org: &str, repo: &str) -> Result<Vec<IdName>> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) {
                     issueTypes(first: 25) { nodes { id name } }
                   }
                 }",
                json!({ "owner": org, "name": repo }),
            )
            .await?;
        parse_at(&data, &["repository", "issueTypes", "nodes"])
    }

    /// Create an issue, returning `(number, url)`. When `project_id` is set
    /// the new issue is added to that ProjectV2 with a follow-up mutation
    /// (`CreateIssueInput` has no ProjectsV2 field).
    pub async fn create_issue(&self, p: &NewIssueParams) -> Result<(u64, String)> {
        let mut input = json!({
            "repositoryId": p.repo_id,
            "title": p.title,
            "body": p.body,
        });
        if !p.assignee_ids.is_empty() {
            input["assigneeIds"] = json!(p.assignee_ids);
        }
        if !p.label_ids.is_empty() {
            input["labelIds"] = json!(p.label_ids);
        }
        if let Some(m) = &p.milestone_id {
            input["milestoneId"] = json!(m);
        }
        if let Some(t) = &p.issue_type_id {
            input["issueTypeId"] = json!(t);
        }
        let data = self
            .graphql(
                "mutation($input: CreateIssueInput!) {
                   createIssue(input: $input) { issue { id number url } }
                 }",
                json!({ "input": input }),
            )
            .await?;
        let issue = data
            .pointer("/createIssue/issue")
            .filter(|v| !v.is_null())
            .ok_or_else(|| GithubError::Shape("createIssue returned no issue".into()))?;
        let issue_id = issue.get("id").and_then(Value::as_str).unwrap_or_default();
        let number = issue.get("number").and_then(Value::as_u64).unwrap_or(0);
        let url = issue
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if let Some(pid) = &p.project_id {
            self.graphql(
                "mutation($projectId: ID!, $contentId: ID!) {
                   addProjectV2ItemById(input: {projectId: $projectId, contentId: $contentId}) {
                     item { id }
                   }
                 }",
                json!({ "projectId": pid, "contentId": issue_id }),
            )
            .await?;
        }
        Ok((number, url))
    }

    pub async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) {
                     labels(first: 100) { nodes { id name } }
                   }
                 }",
                json!({ "owner": org, "name": repo }),
            )
            .await?;
        parse_at(&data, &["repository", "labels", "nodes"])
    }

    /// Fetch a summary of a linked pull request: title/description, state,
    /// review status, checks, the PR's own Actions runs, and recent runs on
    /// the repo's default branch (the "merge to main" runs).
    pub async fn pull_request(&self, pr: &PrRef) -> Result<PrSummary> {
        let data = self
            .graphql(
                PR_SUMMARY_QUERY,
                json!({ "owner": pr.owner, "name": pr.repo, "number": pr.number }),
            )
            .await?;
        if data.get("repository").is_some_and(Value::is_null) {
            return Err(GithubError::Shape(format!(
                "no such repository {}/{}",
                pr.owner, pr.repo
            )));
        }
        let repo: PrRepoResponse = parse_at(&data, &["repository"])?;
        map_pr_summary(pr.clone(), repo)
    }
}

/// True when a GraphQL `errors` array contains a RATE_LIMITED entry.
fn errors_contain_rate_limited(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        arr.iter()
            .any(|e| e.get("type").and_then(Value::as_str) == Some("RATE_LIMITED"))
    })
}

fn parse_at<T: for<'de> Deserialize<'de>>(data: &Value, path: &[&str]) -> Result<T> {
    let mut cur = data;
    for seg in path {
        cur = cur
            .get(seg)
            .ok_or_else(|| GithubError::Shape(format!("missing {}", path.join("."))))?;
    }
    serde_json::from_value(cur.clone()).map_err(|e| GithubError::Shape(e.to_string()))
}

fn login_at(v: &Value, ptr: &str) -> String {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .unwrap_or("ghost")
        .to_string()
}

// ---- response DTOs ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoriesConn {
    page_info: PageInfo,
    nodes: Vec<RepoNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoNode {
    name: String,
    url: String,
    #[serde(default = "default_true")]
    has_issues_enabled: bool,
    issues: IssuesConn,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuesConn {
    page_info: PageInfo,
    nodes: Vec<IssueNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueNode {
    id: String,
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    state: IssueState,
    url: String,
    author: Option<ActorNode>,
    assignees: NamedNodes,
    labels: Option<LabelNodes>,
    comments: CountNode,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    closed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl IssueNode {
    fn to_issue(&self) -> Issue {
        Issue {
            id: self.id.clone(),
            number: self.number,
            title: self.title.clone(),
            body: self.body.clone(),
            state: self.state,
            url: self.url.clone(),
            author: self
                .author
                .as_ref()
                .map(|a| a.login.clone())
                .unwrap_or_else(|| "ghost".into()),
            assignees: self
                .assignees
                .nodes
                .iter()
                .map(|n| n.login.clone())
                .collect(),
            labels: self
                .labels
                .as_ref()
                .map(|l| l.nodes.clone())
                .unwrap_or_default(),
            comment_count: self.comments.total_count,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ActorNode {
    login: String,
}

#[derive(Debug, Deserialize)]
struct IdLogin {
    id: String,
    login: String,
}

impl IdLogin {
    fn into_id_name(self) -> IdName {
        IdName {
            id: self.id,
            name: self.login,
        }
    }
}

#[derive(Debug, Deserialize)]
struct IdTitle {
    id: String,
    title: String,
}

impl IdTitle {
    fn into_id_name(self) -> IdName {
        IdName {
            id: self.id,
            name: self.title,
        }
    }
}

#[derive(Debug, Deserialize)]
struct NamedNodes {
    nodes: Vec<ActorNode>,
}

#[derive(Debug, Deserialize)]
struct LabelNodes {
    nodes: Vec<Label>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountNode {
    total_count: u64,
}

macro_rules! issue_fields {
    () => {
        "id number title body state url
         author { login }
         assignees(first: 10) { nodes { login } }
         labels(first: 20) { nodes { name color } }
         comments { totalCount }
         createdAt updatedAt closedAt"
    };
}

const ORG_ISSUES_QUERY: &str = concat!(
    "query($org: String!, $reposFirst: Int!, $issuesFirst: Int!, $repoCursor: String, $states: [IssueState!]) {
       repositoryOwner(login: $org) {
         repositories(first: $reposFirst, after: $repoCursor, ownerAffiliations: OWNER, isArchived: false, orderBy: {field: NAME, direction: ASC}) {
           pageInfo { hasNextPage endCursor }
           nodes {
             name url hasIssuesEnabled
             issues(first: $issuesFirst, states: $states, orderBy: {field: UPDATED_AT, direction: DESC}) {
               pageInfo { hasNextPage endCursor }
               nodes { ",
    issue_fields!(),
    " }
             }
           }
         }
       }
     }"
);

const REPO_ISSUES_QUERY: &str = concat!(
    "query($owner: String!, $name: String!, $issuesFirst: Int!, $issueCursor: String, $states: [IssueState!]) {
       repository(owner: $owner, name: $name) {
         issues(first: $issuesFirst, after: $issueCursor, states: $states, orderBy: {field: UPDATED_AT, direction: DESC}) {
           pageInfo { hasNextPage endCursor }
           nodes { ",
    issue_fields!(),
    " }
         }
       }
     }"
);

const COMMENTS_QUERY: &str = "
query($id: ID!) {
  node(id: $id) {
    ... on Issue {
      comments(first: 100) {
        nodes { author { login } createdAt body }
      }
    }
  }
}";

const PR_SUMMARY_QUERY: &str = "
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      title
      body
      state
      isDraft
      baseRefName
      headRefName
      additions
      deletions
      changedFiles
      comments { totalCount }
      reviewThreads { totalCount }
      reviewDecision
      reviews(last: 100) { nodes { state author { login } } }
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              state
              contexts(first: 100) {
                nodes {
                  __typename
                  ... on CheckRun { name conclusion }
                  ... on StatusContext { context state }
                }
              }
            }
            checkSuites(first: 10) { nodes { ...CheckSuiteFields } }
          }
        }
      }
    }
    defaultBranchRef {
      name
      target {
        ... on Commit {
          history(first: 5) {
            nodes { checkSuites(first: 10) { nodes { ...CheckSuiteFields } } }
          }
        }
      }
    }
  }
}
fragment CheckSuiteFields on CheckSuite {
  conclusion
  createdAt
  workflowRun { runNumber event workflow { name } }
}";

// ---- pull_request response DTOs ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrRepoResponse {
    pull_request: Option<PullRequestNode>,
    default_branch_ref: Option<DefaultBranchRefNode>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PrStateRaw {
    Open,
    Closed,
    Merged,
}

impl From<PrStateRaw> for PrState {
    fn from(s: PrStateRaw) -> Self {
        match s {
            PrStateRaw::Open => PrState::Open,
            PrStateRaw::Closed => PrState::Closed,
            PrStateRaw::Merged => PrState::Merged,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestNode {
    title: String,
    #[serde(default)]
    body: String,
    state: PrStateRaw,
    is_draft: bool,
    base_ref_name: String,
    head_ref_name: String,
    additions: u64,
    deletions: u64,
    changed_files: u64,
    comments: CountNode,
    review_threads: CountNode,
    review_decision: Option<ReviewDecision>,
    reviews: ReviewsConn,
    commits: PrCommitsConn,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ReviewStateRaw {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

#[derive(Debug, Deserialize)]
struct ReviewsConn {
    nodes: Vec<ReviewNode>,
}

#[derive(Debug, Deserialize)]
struct ReviewNode {
    state: ReviewStateRaw,
    author: Option<ActorNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrCommitsConn {
    nodes: Vec<PrCommitWrapper>,
}

#[derive(Debug, Deserialize)]
struct PrCommitWrapper {
    commit: PrCommitNode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrCommitNode {
    status_check_rollup: Option<StatusCheckRollupNode>,
    check_suites: Option<CheckSuitesConn>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusCheckRollupNode {
    state: Option<String>,
    contexts: ContextsConn,
}

#[derive(Debug, Deserialize)]
struct ContextsConn {
    nodes: Vec<ContextNode>,
}

/// The `CheckRun` / `StatusContext` GraphQL union, flattened into one
/// deserialize target — only the fields for the resolved `__typename` are set.
#[derive(Debug, Deserialize)]
struct ContextNode {
    #[serde(rename = "__typename")]
    typename: String,
    name: Option<String>,
    conclusion: Option<String>,
    context: Option<String>,
    state: Option<String>,
}

impl ContextNode {
    fn into_info(self) -> Option<CheckContextInfo> {
        match self.typename.as_str() {
            "CheckRun" => Some(CheckContextInfo {
                name: self.name.unwrap_or_default(),
                conclusion: self.conclusion.unwrap_or_else(|| "PENDING".into()),
            }),
            "StatusContext" => Some(CheckContextInfo {
                name: self.context.unwrap_or_default(),
                conclusion: self.state.unwrap_or_default(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CheckSuitesConn {
    nodes: Vec<CheckSuiteNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckSuiteNode {
    conclusion: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    workflow_run: Option<WorkflowRunNode>,
}

impl CheckSuiteNode {
    fn into_run_info(self) -> Option<WorkflowRunInfo> {
        let wr = self.workflow_run?;
        Some(WorkflowRunInfo {
            workflow: wr.workflow.name,
            run_number: wr.run_number,
            event: wr.event,
            conclusion: self.conclusion,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRunNode {
    run_number: u64,
    event: String,
    workflow: WorkflowNameNode,
}

#[derive(Debug, Deserialize)]
struct WorkflowNameNode {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultBranchRefNode {
    name: String,
    target: Option<TargetNode>,
}

#[derive(Debug, Deserialize)]
struct TargetNode {
    history: Option<HistoryConn>,
}

#[derive(Debug, Deserialize)]
struct HistoryConn {
    nodes: Vec<HistoryCommitNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryCommitNode {
    check_suites: Option<CheckSuitesConn>,
}

/// Latest review state per author (a reviewer can submit more than one
/// review; only their most recent one counts), plus GitHub's own rollup.
fn summarize_reviews(nodes: Vec<ReviewNode>, decision: Option<ReviewDecision>) -> ReviewSummary {
    use std::collections::HashMap;
    let mut latest: HashMap<String, ReviewStateRaw> = HashMap::new();
    for n in nodes {
        let login = n.author.map(|a| a.login).unwrap_or_else(|| "ghost".into());
        latest.insert(login, n.state);
    }
    let mut summary = ReviewSummary {
        decision,
        ..Default::default()
    };
    for state in latest.values() {
        match state {
            ReviewStateRaw::Approved => summary.approved += 1,
            ReviewStateRaw::ChangesRequested => summary.changes_requested += 1,
            ReviewStateRaw::Commented => summary.commented += 1,
            ReviewStateRaw::Pending | ReviewStateRaw::Dismissed => {}
        }
    }
    summary
}

fn map_pr_summary(pr: PrRef, data: PrRepoResponse) -> Result<PrSummary> {
    let node = data
        .pull_request
        .ok_or_else(|| GithubError::Shape(format!("no such PR {}", pr.label())))?;

    let (checks, pr_runs) = match node.commits.nodes.into_iter().next() {
        Some(PrCommitWrapper { commit }) => {
            let checks = commit
                .status_check_rollup
                .map(|r| CheckRollup {
                    state: r.state,
                    contexts: r
                        .contexts
                        .nodes
                        .into_iter()
                        .filter_map(ContextNode::into_info)
                        .collect(),
                })
                .unwrap_or_default();
            let runs = commit
                .check_suites
                .map(|cs| {
                    cs.nodes
                        .into_iter()
                        .filter_map(CheckSuiteNode::into_run_info)
                        .collect()
                })
                .unwrap_or_default();
            (checks, runs)
        }
        None => (CheckRollup::default(), Vec::new()),
    };

    let (default_branch_name, default_branch_runs) = match data.default_branch_ref {
        Some(dbr) => {
            let runs = dbr
                .target
                .and_then(|t| t.history)
                .map(|h| {
                    h.nodes
                        .into_iter()
                        .filter_map(|n| n.check_suites)
                        .flat_map(|cs| cs.nodes.into_iter())
                        .filter_map(CheckSuiteNode::into_run_info)
                        .collect()
                })
                .unwrap_or_default();
            (dbr.name, runs)
        }
        None => (String::new(), Vec::new()),
    };

    Ok(PrSummary {
        title: node.title,
        body: node.body,
        state: node.state.into(),
        is_draft: node.is_draft,
        base_ref: node.base_ref_name,
        head_ref: node.head_ref_name,
        additions: node.additions,
        deletions: node.deletions,
        changed_files: node.changed_files,
        comment_count: node.comments.total_count,
        review_thread_count: node.review_threads.total_count,
        reviews: summarize_reviews(node.reviews.nodes, node.review_decision),
        checks,
        pr_runs,
        default_branch_name,
        default_branch_runs,
        pr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_node() {
        let raw = serde_json::json!({
            "id": "I_abc", "number": 7, "title": "T", "body": "B",
            "state": "OPEN", "url": "https://github.com/o/r/issues/7",
            "author": {"login": "pgmac"},
            "assignees": {"nodes": [{"login": "pgmac"}]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}]},
            "comments": {"totalCount": 3},
            "createdAt": "2026-07-01T00:00:00Z",
            "updatedAt": "2026-07-02T00:00:00Z",
            "closedAt": null
        });
        let node: IssueNode = serde_json::from_value(raw).unwrap();
        let issue = node.to_issue();
        assert_eq!(issue.number, 7);
        assert_eq!(issue.author, "pgmac");
        assert_eq!(issue.assignees, vec!["pgmac"]);
        assert_eq!(issue.labels[0].name, "bug");
        assert_eq!(issue.comment_count, 3);
        assert!(issue.closed_at.is_none());
    }

    #[test]
    fn deleted_author_becomes_ghost() {
        let raw = serde_json::json!({
            "id": "I_x", "number": 1, "title": "t", "body": "",
            "state": "CLOSED", "url": "u",
            "author": null,
            "assignees": {"nodes": []},
            "labels": {"nodes": []},
            "comments": {"totalCount": 0},
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "closedAt": "2026-01-02T00:00:00Z"
        });
        let node: IssueNode = serde_json::from_value(raw).unwrap();
        assert_eq!(node.to_issue().author, "ghost");
    }

    #[test]
    fn pagination_shape_parses() {
        let raw = serde_json::json!({
            "pageInfo": {"hasNextPage": true, "endCursor": "abc"},
            "nodes": []
        });
        let conn: IssuesConn = serde_json::from_value(raw).unwrap();
        assert!(conn.page_info.has_next_page);
        assert_eq!(conn.page_info.end_cursor.as_deref(), Some("abc"));
    }

    #[test]
    fn rate_limited_graphql_error_detected() {
        let errors = serde_json::json!([
            {"type": "RATE_LIMITED", "message": "API rate limit exceeded for user"}
        ]);
        assert!(errors_contain_rate_limited(&errors));

        let other = serde_json::json!([{"type": "NOT_FOUND", "message": "nope"}]);
        assert!(!errors_contain_rate_limited(&other));

        let untyped = serde_json::json!([{"message": "boom"}]);
        assert!(!errors_contain_rate_limited(&untyped));
    }

    #[test]
    fn queries_request_all_issue_fields() {
        for field in [
            "createdAt",
            "updatedAt",
            "closedAt",
            "totalCount",
            "assignees",
        ] {
            assert!(ORG_ISSUES_QUERY.contains(field));
            assert!(REPO_ISSUES_QUERY.contains(field));
        }
    }

    #[test]
    fn issues_query_supports_user_and_org_owners() {
        assert!(ORG_ISSUES_QUERY.contains("repositoryOwner(login: $org)"));
        assert!(!ORG_ISSUES_QUERY.contains("organization(login:"));
        // Without OWNER affiliation a user login also lists repos they merely
        // collaborate on or reach via org membership.
        assert!(ORG_ISSUES_QUERY.contains("ownerAffiliations: OWNER"));
    }

    fn sample_pr_ref() -> PrRef {
        PrRef {
            owner: "pgmac-net".into(),
            repo: "gh-issues-tui".into(),
            number: 72,
        }
    }

    #[test]
    fn pr_summary_parses_mixed_check_union_and_reviews() {
        let raw = serde_json::json!({
            "pullRequest": {
                "title": "Add PR summary",
                "body": "closes #45",
                "state": "OPEN",
                "isDraft": false,
                "baseRefName": "main",
                "headRefName": "45-pr-link-summary",
                "additions": 120,
                "deletions": 8,
                "changedFiles": 5,
                "comments": {"totalCount": 3},
                "reviewThreads": {"totalCount": 1},
                "reviewDecision": "APPROVED",
                "reviews": {
                    "nodes": [
                        {"state": "CHANGES_REQUESTED", "author": {"login": "alice"}},
                        {"state": "APPROVED", "author": {"login": "alice"}},
                        {"state": "COMMENTED", "author": {"login": "bob"}}
                    ]
                },
                "commits": {
                    "nodes": [{
                        "commit": {
                            "statusCheckRollup": {
                                "state": "SUCCESS",
                                "contexts": {
                                    "nodes": [
                                        {"__typename": "CheckRun", "name": "ci", "conclusion": "SUCCESS", "context": null, "state": null},
                                        {"__typename": "StatusContext", "name": null, "conclusion": null, "context": "legacy-ci", "state": "SUCCESS"}
                                    ]
                                }
                            },
                            "checkSuites": {
                                "nodes": [{
                                    "conclusion": "SUCCESS",
                                    "createdAt": "2026-07-20T00:00:00Z",
                                    "workflowRun": {"runNumber": 42, "event": "pull_request", "workflow": {"name": "ci.yml"}}
                                }]
                            }
                        }
                    }]
                }
            },
            "defaultBranchRef": {
                "name": "main",
                "target": {
                    "history": {
                        "nodes": [{
                            "checkSuites": {
                                "nodes": [{
                                    "conclusion": "SUCCESS",
                                    "createdAt": "2026-07-19T00:00:00Z",
                                    "workflowRun": {"runNumber": 128, "event": "push", "workflow": {"name": "release.yml"}}
                                }]
                            }
                        }]
                    }
                }
            }
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        let summary = map_pr_summary(sample_pr_ref(), repo).unwrap();

        assert_eq!(summary.state, PrState::Open);
        assert!(!summary.is_draft);
        assert_eq!(summary.additions, 120);
        // alice's later APPROVED review overrides her earlier CHANGES_REQUESTED.
        assert_eq!(summary.reviews.approved, 1);
        assert_eq!(summary.reviews.changes_requested, 0);
        assert_eq!(summary.reviews.commented, 1);
        assert_eq!(summary.reviews.decision, Some(ReviewDecision::Approved));

        let checks = &summary.checks;
        assert_eq!(checks.state.as_deref(), Some("SUCCESS"));
        assert_eq!(checks.contexts.len(), 2);
        assert_eq!(checks.contexts[0].name, "ci");
        assert_eq!(checks.contexts[1].name, "legacy-ci");

        assert_eq!(summary.pr_runs.len(), 1);
        assert_eq!(summary.pr_runs[0].workflow, "ci.yml");
        assert_eq!(summary.default_branch_name, "main");
        assert_eq!(summary.default_branch_runs.len(), 1);
        assert_eq!(summary.default_branch_runs[0].workflow, "release.yml");
    }

    #[test]
    fn pr_summary_handles_empty_rollup_and_no_default_branch() {
        let raw = serde_json::json!({
            "pullRequest": {
                "title": "Draft PR",
                "body": "",
                "state": "OPEN",
                "isDraft": true,
                "baseRefName": "main",
                "headRefName": "feature",
                "additions": 0,
                "deletions": 0,
                "changedFiles": 0,
                "comments": {"totalCount": 0},
                "reviewThreads": {"totalCount": 0},
                "reviewDecision": null,
                "reviews": {"nodes": []},
                "commits": {"nodes": []}
            },
            "defaultBranchRef": null
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        let summary = map_pr_summary(sample_pr_ref(), repo).unwrap();

        assert!(summary.is_draft);
        assert!(summary.checks.contexts.is_empty());
        assert!(summary.checks.state.is_none());
        assert!(summary.pr_runs.is_empty());
        assert!(summary.default_branch_runs.is_empty());
        assert_eq!(summary.default_branch_name, "");
        assert_eq!(summary.reviews.decision, None);
    }

    #[test]
    fn pr_summary_query_requests_key_fields() {
        for field in [
            "reviewDecision",
            "statusCheckRollup",
            "checkSuites",
            "defaultBranchRef",
            "CheckRun",
            "StatusContext",
        ] {
            assert!(PR_SUMMARY_QUERY.contains(field), "missing {field}");
        }
    }
}
