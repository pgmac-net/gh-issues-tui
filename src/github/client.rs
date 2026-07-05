use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::error::{GithubError, RATE_LIMIT_MSG_PREFIX, Result};
use super::types::{Comment, Issue, IssueState, Label, RateLimitData, RepoIssues, RepoLabel};

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
            .user_agent(concat!("gh-issues-tui/", env!("CARGO_PKG_VERSION")))
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

    /// Fetch all issues for every repository in the organisation.
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

            let repos: RepositoriesConn = parse_at(&data, &["organization", "repositories"])?;
            for repo in repos.nodes {
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

                if !issues.is_empty() {
                    out.push(RepoIssues {
                        repo: repo.name,
                        repo_url: repo.url,
                        issues,
                    });
                }
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
struct RepoNode {
    name: String,
    url: String,
    issues: IssuesConn,
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
       organization(login: $org) {
         repositories(first: $reposFirst, after: $repoCursor, orderBy: {field: NAME, direction: ASC}) {
           pageInfo { hasNextPage endCursor }
           nodes {
             name url
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
}
