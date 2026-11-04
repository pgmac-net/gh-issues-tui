use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::provider::error::{ProviderError, Result, found};
use crate::provider::http::{
    RateLimitStore, build_http_client, graphql_body, graphql_data, header_i64, join_error_messages,
    parse_at,
};
use crate::provider::scope::FetchScope;
use crate::provider::types::{
    CheckContextInfo, CheckRollup, Comment, Created, FormOptions, IdName, Issue, IssueRef,
    IssueState, IssueStub, Label, NewIssueParams, PrLookup, PrRef, PrState, PrSummary,
    RateLimitData, Relations, RepoIssues, RepoLabel, ReviewDecision, ReviewSummary,
    SubIssueProgress, WorkflowRunInfo,
};
use crate::runs::{ActionsJob, ActionsRun, LinkedPr};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
/// GitHub's maximum page size. Cost does not scale with this once the bulk
/// query carries no nested connections, so the largest page is also the
/// cheapest: fewer requests for the same points.
const REPOS_PAGE: u32 = 100;
const ISSUES_PAGE: u32 = 100;
/// GitHub rejects `nodes(ids:)` with more than 100 ids.
const HYDRATE_BATCH: usize = 100;
const MIN_REPOS_PAGE: u32 = 5;
const MIN_ISSUES_PAGE: u32 = 10;

/// Page sizes for the org-issues fetch, shrunk on a `ResourceLimited` error
/// and never grown back — a query that overflows GitHub's complexity budget
/// once is likely to again, so later requests in the same fetch start small.
#[derive(Debug, Clone, Copy)]
struct PageSizes {
    repos: u32,
    issues: u32,
}

impl PageSizes {
    /// Halves both dimensions toward their floors. Returns `false` (no
    /// change made) once both are already at the floor — the caller should
    /// give up and surface the underlying error at that point.
    fn shrink(&mut self) -> bool {
        let next_repos = halve(self.repos, MIN_REPOS_PAGE);
        let next_issues = halve(self.issues, MIN_ISSUES_PAGE);
        if next_repos == self.repos && next_issues == self.issues {
            return false;
        }
        self.repos = next_repos;
        self.issues = next_issues;
        true
    }
}

fn halve(current: u32, floor: u32) -> u32 {
    if current <= floor {
        floor
    } else {
        (current / 2).max(floor)
    }
}

/// Async GitHub GraphQL v4 client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    pub(super) http: reqwest::Client,
    rate_limit: RateLimitStore,
    /// REST's budget, separate from GraphQL's (`github::actions`).
    pub(super) rest_rate_limit: RateLimitStore,
}

impl Client {
    pub fn new(token: String) -> anyhow::Result<Self> {
        Ok(Self {
            http: build_http_client(&format!("Bearer {token}"), &[])?,
            rate_limit: RateLimitStore::default(),
            rest_rate_limit: RateLimitStore::default(),
        })
    }

    /// Returns the most recently observed rate limit state.
    pub fn rate_limit(&self) -> Option<RateLimitData> {
        self.rate_limit.get()
    }

    /// GitHub reports the reset as epoch **seconds**, so it is stored as-is.
    fn update_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        self.note_rate_limit(&self.rate_limit, headers);
    }

    /// Record the rate-limit headers of a response into `store`.
    pub(super) fn note_rate_limit(
        &self,
        store: &RateLimitStore,
        headers: &reqwest::header::HeaderMap,
    ) {
        let remaining = header_i64(headers, "x-ratelimit-remaining");
        let limit = header_i64(headers, "x-ratelimit-limit");
        let reset = header_i64(headers, "x-ratelimit-reset");
        if let (Some(remaining), Some(limit), Some(reset)) = (remaining, limit, reset) {
            store.set(RateLimitData {
                remaining: remaining.max(0) as u64,
                limit: limit.max(0) as u64,
                reset,
                last_cost: None,
            });
        }
    }

    pub(super) async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let resp = self
            .http
            .post(GRAPHQL_URL)
            .json(&graphql_body(query, variables))
            .send()
            .await?;

        self.update_rate_limit(resp.headers());

        let status = resp.status();
        if (status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            && let Some(data) = self.rate_limit.get()
            && data.remaining == 0
        {
            return Err(ProviderError::RateLimited(self.rate_limit.message(None)));
        }

        let body: Value = resp.error_for_status()?.json().await?;
        let data = graphql_data(&body, |errors| {
            // The GraphQL API reports the primary rate limit as an HTTP 200
            // with a RATE_LIMITED error entry, not as a 403.
            if errors_contain_rate_limited(errors) {
                return Some(ProviderError::RateLimited(
                    self.rate_limit.message(Some("GraphQL")),
                ));
            }
            // Not a rate limit, but still not a plain API error — the caller
            // retries this one with smaller pages.
            if errors_contain_resource_limited(errors) {
                return Some(ProviderError::ResourceLimited(join_error_messages(errors)));
            }
            if errors_all_not_found(errors) {
                return Some(ProviderError::NotFound(join_error_messages(errors)));
            }
            None
        })?;

        // Queries that ask for `rateLimit { cost }` report what they actually
        // cost; the rest simply don't contribute to the running total.
        if let Some(cost) = data.pointer("/rateLimit/cost").and_then(Value::as_u64) {
            self.rate_limit.add_cost(cost);
        }
        Ok(data)
    }

    /// Runs a GraphQL query built by `make_vars` from the current page
    /// sizes, retrying with smaller pages on a `ResourceLimited` error.
    /// GraphQL cursors are opaque positions, so resuming with a smaller
    /// `first` from the same cursor is valid — the full dataset still
    /// arrives, just across more requests.
    async fn graphql_with_backoff(
        &self,
        query: &str,
        mut make_vars: impl FnMut(&PageSizes) -> Value,
        sizes: &mut PageSizes,
    ) -> Result<Value> {
        loop {
            match self.graphql(query, make_vars(sizes)).await {
                Err(ProviderError::ResourceLimited(msg)) => {
                    if !sizes.shrink() {
                        return Err(ProviderError::ResourceLimited(msg));
                    }
                }
                other => return other,
            }
        }
    }

    /// Fetch all issues for every repository owned by `org` — an
    /// organisation or a user account (`repositoryOwner` covers both).
    ///
    /// Iterates repositories with cursor pagination (avoids the 1000-result
    /// search API cap) and follows per-repo issue pagination where needed.
    pub async fn org_issues(&self, org: &str, scope: &FetchScope) -> Result<Vec<RepoIssues>> {
        match self.org_issues_at(org, &issue_filter(scope)).await {
            Err(e) if scope.assignee.is_some() && is_unknown_assignee(&e) => {
                let fallback = without_assignee(scope);
                self.org_issues_at(org, &issue_filter(&fallback)).await
            }
            other => other,
        }
    }

    /// [`Self::org_issues`] with the `filterBy` value already built.
    async fn org_issues_at(&self, org: &str, filter: &Value) -> Result<Vec<RepoIssues>> {
        let mut out = Vec::new();
        let mut repo_cursor: Option<String> = None;
        let mut sizes = PageSizes {
            repos: REPOS_PAGE,
            issues: ISSUES_PAGE,
        };
        self.rate_limit.begin_fetch();

        loop {
            let data = self
                .graphql_with_backoff(
                    ORG_ISSUES_QUERY,
                    |sizes| {
                        json!({
                            "org": org,
                            "reposFirst": sizes.repos,
                            "issuesFirst": sizes.issues,
                            "repoCursor": repo_cursor,
                            "filter": filter,
                        })
                    },
                    &mut sizes,
                )
                .await?;

            // An unknown login yields `"repositoryOwner": null`, not an error.
            if data.get("repositoryOwner").is_some_and(Value::is_null) {
                return Err(ProviderError::Shape(format!("no such org or user: {org}")));
            }
            let repos: RepositoriesConn = parse_at(&data, &["repositoryOwner", "repositories"])?;
            for repo in repos.nodes {
                // Repos with issues disabled can never hold issues — skip.
                if !repo.has_issues_enabled {
                    continue;
                }
                // Empty repos are kept: the hide-empty-repos filter decides
                // their visibility client-side, so toggling needs no refetch.
                out.push(self.finish_repo(org, repo, filter, &mut sizes).await?);
            }

            if !repos.page_info.has_next_page {
                break;
            }
            repo_cursor = repos.page_info.end_cursor;
        }

        // Hydration is deliberately *not* interleaved with the pages above:
        // one pass over the finished set batches ids maximally, and callers
        // get fully-populated issues or an error, never a half-labelled list.
        // `provider::priority` derives priority from labels and the label
        // filters read them, so rows must never be built from empty sets.
        self.hydrate_issues(&mut out).await?;
        self.rate_limit.end_fetch();
        Ok(out)
    }

    /// One repository's issues, shaped like its `org_issues` entry (#229).
    /// `Ok(None)` for the repos `org_issues` skips (archived, issues
    /// disabled) and for one that no longer exists.
    pub async fn repo_issues(
        &self,
        org: &str,
        repo: &str,
        scope: &FetchScope,
    ) -> Result<Option<RepoIssues>> {
        match self.repo_issues_at(org, repo, &issue_filter(scope)).await {
            Err(e) if scope.assignee.is_some() && is_unknown_assignee(&e) => {
                let fallback = without_assignee(scope);
                self.repo_issues_at(org, repo, &issue_filter(&fallback))
                    .await
            }
            other => other,
        }
    }

    /// [`Self::repo_issues`] with the `filterBy` value already built.
    async fn repo_issues_at(
        &self,
        org: &str,
        repo: &str,
        filter: &Value,
    ) -> Result<Option<RepoIssues>> {
        let mut sizes = PageSizes {
            repos: REPOS_PAGE,
            issues: ISSUES_PAGE,
        };
        let data = found(
            self.graphql_with_backoff(
                REPO_QUERY,
                |sizes| {
                    json!({
                        "owner": org,
                        "name": repo,
                        "issuesFirst": sizes.issues,
                        "filter": filter,
                    })
                },
                &mut sizes,
            )
            .await,
        )?;
        let Some(node) = data
            .as_ref()
            .and_then(|d| d.get("repository"))
            .filter(|r| !r.is_null())
        else {
            return Ok(None);
        };
        let node: RepoNode = serde_json::from_value(node.clone())
            .map_err(|e| ProviderError::Shape(e.to_string()))?;
        if node.is_archived || !node.has_issues_enabled {
            return Ok(None);
        }
        let mut out = vec![self.finish_repo(org, node, filter, &mut sizes).await?];
        self.hydrate_issues(&mut out).await?;
        Ok(out.pop())
    }

    /// Names of the repos `org_issues` would return, without issues (#230):
    /// owned, not archived, issues enabled. One cheap page per 100 repos.
    pub async fn repo_names(&self, org: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let data = self
                .graphql(REPO_NAMES_QUERY, json!({ "org": org, "cursor": cursor }))
                .await?;
            if data.get("repositoryOwner").is_some_and(Value::is_null) {
                return Err(ProviderError::Shape(format!("no such org or user: {org}")));
            }
            let conn: RepoNamesConn = parse_at(&data, &["repositoryOwner", "repositories"])?;
            names.extend(
                conn.nodes
                    .into_iter()
                    .filter(|r| r.has_issues_enabled)
                    .map(|r| r.name),
            );
            if !conn.page_info.has_next_page {
                return Ok(names);
            }
            cursor = conn.page_info.end_cursor;
        }
    }

    /// One issue by node id with its labels and assignees (#229). `Ok(None)`
    /// when GitHub no longer resolves the id (deleted, transferred, hidden).
    pub async fn issue(&self, issue_id: &str) -> Result<Option<Issue>> {
        let data = found(
            self.graphql(ISSUE_NODE_QUERY, json!({ "id": issue_id }))
                .await,
        )?;
        match data
            .as_ref()
            .and_then(|d| d.get("node"))
            .filter(|n| !n.is_null())
        {
            None => Ok(None),
            Some(node) => serde_json::from_value::<MutatedIssue>(node.clone())
                .map(|m| Some(m.into_issue()))
                .map_err(|e| ProviderError::Shape(e.to_string())),
        }
    }

    /// Turn a repository node from a bulk or single-repo page into its
    /// `RepoIssues`, following issue pagination past the first page.
    async fn finish_repo(
        &self,
        owner: &str,
        repo: RepoNode,
        filter: &Value,
        sizes: &mut PageSizes,
    ) -> Result<RepoIssues> {
        let mut issues: Vec<Issue> = repo.issues.nodes.iter().map(IssueNode::to_issue).collect();
        let mut page = repo.issues.page_info;
        while page.has_next_page {
            let data = self
                .graphql_with_backoff(
                    REPO_ISSUES_QUERY,
                    |sizes| {
                        json!({
                            "owner": owner,
                            "name": repo.name,
                            "issuesFirst": sizes.issues,
                            "issueCursor": page.end_cursor,
                            "filter": filter,
                        })
                    },
                    sizes,
                )
                .await?;
            let conn: IssuesConn = parse_at(&data, &["repository", "issues"])?;
            issues.extend(conn.nodes.iter().map(IssueNode::to_issue));
            page = conn.page_info;
        }
        Ok(RepoIssues {
            repo: repo.name,
            repo_url: repo.url,
            issues,
        })
    }

    /// Fill in `labels` and `assignees`, which [`issue_fields`] deliberately
    /// omits, via `nodes(ids:)` batches of 100 (GitHub's per-call id limit).
    ///
    /// Measured cost is 2 points per batch, against 100 points for carrying
    /// the same two connections inline in the bulk query.
    async fn hydrate_issues(&self, repos: &mut [RepoIssues]) -> Result<()> {
        let ids: Vec<String> = repos
            .iter()
            .flat_map(|r| r.issues.iter().map(|i| i.id.clone()))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }

        let mut hydrated: HashMap<String, HydratedIssue> = HashMap::new();
        for chunk in ids.chunks(HYDRATE_BATCH) {
            let data = self
                .graphql(ISSUE_HYDRATE_QUERY, json!({ "ids": chunk }))
                .await?;
            let nodes = data
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| ProviderError::Shape("missing nodes".into()))?;
            for node in nodes {
                // A deleted or type-mismatched id comes back as null, and an
                // id the viewer can no longer see yields an empty object —
                // both leave the issue with the empty sets it already has.
                if node.is_null() {
                    continue;
                }
                let issue: HydratedIssue = serde_json::from_value(node.clone())
                    .map_err(|e| ProviderError::Shape(e.to_string()))?;
                hydrated.insert(issue.id.clone(), issue);
            }
        }

        for repo in repos.iter_mut() {
            for issue in repo.issues.iter_mut() {
                if let Some(h) = hydrated.get(&issue.id) {
                    issue.labels = h.labels.nodes.clone();
                    issue.assignees = h.assignees.nodes.iter().map(|n| n.login.clone()).collect();
                }
            }
        }
        Ok(())
    }

    /// Fetch the comment thread of an issue by node id.
    pub async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>> {
        let data = self
            .graphql(COMMENTS_QUERY, json!({ "id": issue_id }))
            .await?;
        let nodes = data
            .pointer("/node/comments/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Shape("missing comments".into()))?;
        nodes
            .iter()
            .map(|n| {
                Ok(Comment {
                    id: n["id"].as_str().unwrap_or_default().to_string(),
                    author: login_at(n, "/author/login"),
                    created_at: serde_json::from_value(n["createdAt"].clone())
                        .map_err(|e| ProviderError::Shape(e.to_string()))?,
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

    pub async fn update_comment(&self, comment_id: &str, body: &str) -> Result<()> {
        self.graphql(
            "mutation($id: ID!, $body: String!) {
               updateIssueComment(input: {id: $id, body: $body}) { clientMutationId }
             }",
            json!({ "id": comment_id, "body": body }),
        )
        .await
        .map(drop)
    }

    pub async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<Issue> {
        let (q, field) = match state {
            IssueState::Closed => (CLOSE_ISSUE_MUTATION, "closeIssue"),
            IssueState::Open => (REOPEN_ISSUE_MUTATION, "reopenIssue"),
        };
        let data = self.graphql(q, json!({ "id": issue_id })).await?;
        issue_from_payload(&data, field)
    }

    pub async fn update_title(&self, issue_id: &str, title: &str) -> Result<Issue> {
        self.update_issue(json!({ "id": issue_id, "title": title }))
            .await
    }

    pub async fn update_body(&self, issue_id: &str, body: &str) -> Result<Issue> {
        self.update_issue(json!({ "id": issue_id, "body": body }))
            .await
    }

    /// Run `updateIssue` with `input` and return the issue it reports. One
    /// mutation document serves every field: GitHub leaves an `UpdateIssueInput`
    /// field untouched when it is absent from `input`.
    async fn update_issue(&self, input: Value) -> Result<Issue> {
        let data = self
            .graphql(UPDATE_ISSUE_MUTATION, json!({ "input": input }))
            .await?;
        issue_from_payload(&data, "updateIssue")
    }

    /// Replace the full assignee set. Logins are resolved to user ids.
    pub async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<Issue> {
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
                .ok_or_else(|| ProviderError::Shape(format!("unknown user {login}")))?
                .to_string();
            ids.push(id);
        }
        self.update_issue(json!({ "id": issue_id, "assigneeIds": ids }))
            .await
    }

    /// Replace the full label set with labels named in `names` (must exist on the repo).
    pub async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<Issue> {
        let all = self.repo_labels(org, repo).await?;
        let mut ids = Vec::new();
        for name in names {
            let label = all
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| ProviderError::Shape(format!("no label named {name} in {repo}")))?;
            ids.push(label.id.clone());
        }
        self.update_issue(json!({ "id": issue_id, "labelIds": ids }))
            .await
    }

    /// Move the issue to another repo in the same org. Missing labels are
    /// recreated in the target — `createLabelsIfMissing: true` — so
    /// `priority:*`/`status:*`, which this app's sort/colour/filter model
    /// depends on, survive the transfer instead of being silently dropped.
    /// GitHub only allows same-owner transfers, so `target` is always one
    /// of the org's already-loaded repos.
    ///
    /// Returns the issue as it is after the move (#226), so the list patches
    /// it in place instead of refetching the org. A transfer gives the issue a
    /// new node id and number in the target repo.
    pub async fn move_issue(&self, issue_id: &str, org: &str, target: &str) -> Result<Issue> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) { id }
                 }",
                json!({ "owner": org, "name": target }),
            )
            .await?;
        let repo_id = data
            .pointer("/repository/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape(format!("no repository {org}/{target}")))?
            .to_string();
        let data = self
            .graphql(
                TRANSFER_ISSUE_MUTATION,
                json!({ "id": issue_id, "repo": repo_id }),
            )
            .await?;
        issue_from_payload(&data, "transferIssue")
    }

    /// Everything the new-issue form's pickers need, in one query (plus
    /// separate, failure-tolerant issue-types and projects queries — issue
    /// types are an org feature not available for every owner, and projects
    /// need the `read:project` scope; neither may kill the form, #253).
    pub async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) {
                     id
                     labels(first: 100, orderBy: {field: NAME, direction: ASC}) { nodes { id name } }
                     assignableUsers(first: 100) { nodes { id login } }
                     milestones(first: 50, states: [OPEN], orderBy: {field: DUE_DATE, direction: ASC}) { nodes { id title } }
                   }
                 }",
                json!({ "owner": org, "name": repo }),
            )
            .await?;
        if data.get("repository").is_some_and(Value::is_null) {
            return Err(ProviderError::Shape(format!("no repository {org}/{repo}")));
        }
        let repo_id = data
            .pointer("/repository/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Shape("missing repository.id".into()))?
            .to_string();

        let labels: Vec<IdName> = parse_at(&data, &["repository", "labels", "nodes"])?;
        let users: Vec<IdLogin> = parse_at(&data, &["repository", "assignableUsers", "nodes"])?;
        let milestones: Vec<IdTitle> = parse_at(&data, &["repository", "milestones", "nodes"])?;

        let issue_types = self.issue_types(org, repo).await.unwrap_or_default();
        let (projects, projects_unavailable) = match self.repo_projects(org, repo).await {
            Ok(p) => (p, None),
            Err(e) => (Vec::new(), Some(scope_reason(&e, "read:project"))),
        };

        Ok(FormOptions {
            repo_id,
            labels,
            users: users.into_iter().map(IdLogin::into_id_name).collect(),
            milestones: milestones.into_iter().map(IdTitle::into_id_name).collect(),
            projects: projects.into_iter().map(IdTitle::into_id_name).collect(),
            projects_unavailable,
            issue_types,
        })
    }

    /// The repo's ProjectsV2, in a query of their own: the `projectsV2` field
    /// needs the `read:project` scope, and GitHub fails a whole query that
    /// asks for a field the token cannot read (#253).
    async fn repo_projects(&self, org: &str, repo: &str) -> Result<Vec<IdTitle>> {
        let data = self
            .graphql(
                "query($owner: String!, $name: String!) {
                   repository(owner: $owner, name: $name) {
                     projectsV2(first: 50) { nodes { id title } }
                   }
                 }",
                json!({ "owner": org, "name": repo }),
            )
            .await?;
        parse_at(&data, &["repository", "projectsV2", "nodes"])
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

    /// Create an issue, returning it as the backend holds it after the write
    /// (#249), so the list can show it without a refetch. When `project_id`
    /// is set the new issue is added to that ProjectV2 with a follow-up
    /// mutation (`CreateIssueInput` has no ProjectsV2 field).
    pub async fn create_issue(&self, p: &NewIssueParams) -> Result<Created> {
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
            .graphql(CREATE_ISSUE_MUTATION, json!({ "input": input }))
            .await?;
        let issue = issue_from_payload(&data, "createIssue")?;
        let issue_id = issue.id.clone();

        // The issue exists from here on, so a failed project add is a warning
        // on a successful create, not a failed create (#254).
        let mut warning = None;
        if let Some(pid) = &p.project_id
            && let Err(e) = self
                .graphql(
                    "mutation($projectId: ID!, $contentId: ID!) {
                       addProjectV2ItemById(input: {projectId: $projectId, contentId: $contentId}) {
                         item { id }
                       }
                     }",
                    json!({ "projectId": pid, "contentId": issue_id }),
                )
                .await
        {
            warning = Some(scope_reason(&e, "project"));
        }
        Ok(Created { issue, warning })
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
    pub async fn pull_request(&self, pr: &PrRef) -> Result<PrLookup> {
        let data = self
            .graphql(
                PR_SUMMARY_QUERY,
                json!({ "owner": pr.owner, "name": pr.repo, "number": pr.number }),
            )
            .await?;
        if data.get("repository").is_some_and(Value::is_null) {
            return Err(ProviderError::Shape(format!(
                "no such repository {}/{}",
                pr.owner, pr.repo
            )));
        }
        let repo: PrRepoResponse = parse_at(&data, &["repository"])?;
        map_pr_lookup(pr.clone(), repo)
    }
}

/// True when a GraphQL `errors` array contains a RATE_LIMITED entry.
fn errors_contain_rate_limited(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        arr.iter()
            .any(|e| e.get("type").and_then(Value::as_str) == Some("RATE_LIMITED"))
    })
}

/// True when a GraphQL `errors` array contains a query-too-complex entry.
/// GitHub's error `type` for this case isn't consistently documented, so
/// this also matches on the message text observed in practice.
fn errors_contain_resource_limited(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        arr.iter().any(|e| {
            let type_match = e.get("type").and_then(Value::as_str).is_some_and(|t| {
                matches!(
                    t.to_ascii_uppercase().as_str(),
                    "EXCESSIVE_RESOURCE_USAGE" | "RESOURCE_LIMITED" | "MAX_NODE_LIMIT_EXCEEDED"
                )
            });
            let msg_match = e
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("Resource limits"));
            type_match || msg_match
        })
    })
}

/// True when every GraphQL error is `NOT_FOUND`: the thing asked for is gone
/// or hidden, and nothing else went wrong (#229).
fn errors_all_not_found(errors: &Value) -> bool {
    errors.as_array().is_some_and(|arr| {
        !arr.is_empty()
            && arr
                .iter()
                .all(|e| e.get("type").and_then(Value::as_str) == Some("NOT_FOUND"))
    })
}

/// `["OPEN"]`, or both states once closed issues are wanted.
fn issue_states(include_closed: bool) -> Vec<&'static str> {
    if include_closed {
        vec!["OPEN", "CLOSED"]
    } else {
        vec!["OPEN"]
    }
}

/// The `IssueFilters` value for `scope` (#230): states always, the rest only
/// when set. Every field is case-insensitive at GitHub (checked live), which
/// matches the client-side filters it pre-empts.
fn issue_filter(scope: &FetchScope) -> Value {
    let mut f = json!({ "states": issue_states(scope.include_closed) });
    if let Some(login) = &scope.assignee {
        f["assignee"] = json!(login);
    }
    if let Some(login) = &scope.author {
        f["createdBy"] = json!(login);
    }
    if let Some(since) = scope.updated_since {
        f["since"] = json!(since.to_rfc3339());
    }
    if !scope.labels.is_empty() {
        f["labels"] = json!(scope.labels);
    }
    f
}

/// GitHub fails the whole query when `filterBy.assignee` names a login that
/// does not exist (`UNPROCESSABLE`), where a real non-collaborator just
/// matches nothing (both checked live). `createdBy` never errors.
fn is_unknown_assignee(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Api(m) if m.contains("Could not find an assignee"))
}

/// `scope` without its assignee: a superset, so still correct to filter
/// client-side, for retrying after [`is_unknown_assignee`].
fn without_assignee(scope: &FetchScope) -> FetchScope {
    FetchScope {
        assignee: None,
        ..scope.clone()
    }
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
    /// Only the single-repo query asks; the bulk query excludes archived
    /// repos with `isArchived: false` instead.
    #[serde(default)]
    is_archived: bool,
    issues: IssuesConn,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoNamesConn {
    page_info: PageInfo,
    nodes: Vec<RepoNameNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoNameNode {
    name: String,
    #[serde(default = "default_true")]
    has_issues_enabled: bool,
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
    comments: CountNode,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    closed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The parent chain (#234): objects, not connections, so free in the
    /// bulk query. Null for a top-level issue or a parent the viewer can't see.
    #[serde(default)]
    parent: Option<Box<ParentNode>>,
    #[serde(default)]
    sub_issues_summary: Option<SubIssuesSummaryNode>,
    #[serde(default)]
    issue_dependencies_summary: Option<DependenciesNode>,
    /// `relatesTo { totalCount }`: a connection, but with no `first:` it is
    /// not billed per issue slot (measured: still cost 1).
    #[serde(default)]
    relates_to: Option<CountNode>,
}

/// One link of an issue's parent chain, as [`ancestor_fields`] selects it.
#[derive(Debug, Deserialize)]
struct ParentNode {
    id: String,
    number: u64,
    title: String,
    state: IssueState,
    url: String,
    repository: ParentRepo,
    #[serde(default)]
    parent: Option<Box<ParentNode>>,
}

#[derive(Debug, Deserialize)]
struct ParentRepo {
    name: String,
    owner: ActorNode,
}

#[derive(Debug, Deserialize)]
struct SubIssuesSummaryNode {
    completed: u32,
    total: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DependenciesNode {
    blocked_by: u32,
    blocking: u32,
}

impl IssueNode {
    /// Flatten the nested parent chain, nearest first.
    fn ancestors(&self) -> Vec<IssueStub> {
        let mut out = Vec::new();
        let mut next = self.parent.as_deref();
        while let Some(p) = next {
            out.push(IssueStub {
                id: p.id.clone(),
                number: p.number,
                title: p.title.clone(),
                state: p.state,
                url: p.url.clone(),
                repo: p.repository.name.clone(),
                owner: p.repository.owner.login.clone(),
            });
            next = p.parent.as_deref();
        }
        out
    }

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
            // Filled in by `hydrate_issues`, which fetches both in one batched
            // `nodes(ids:)` request rather than as connections nested in the
            // bulk query.
            assignees: Vec::new(),
            labels: Vec::new(),
            comment_count: self.comments.total_count,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            ancestors: self.ancestors(),
            sub_issues: self
                .sub_issues_summary
                .as_ref()
                .filter(|s| s.total > 0)
                .map(|s| SubIssueProgress {
                    completed: s.completed,
                    total: s.total,
                }),
            relations: {
                let deps = self.issue_dependencies_summary.as_ref();
                let r = Relations {
                    blocked_by: deps.map_or(0, |d| d.blocked_by),
                    blocking: deps.map_or(0, |d| d.blocking),
                    related: self.relates_to.as_ref().map_or(0, |c| c.total_count as u32),
                };
                (r != Relations::default()).then_some(r)
            },
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

#[derive(Debug, Default, Deserialize)]
struct NamedNodes {
    nodes: Vec<ActorNode>,
}

/// One `nodes(ids:)` entry: the fields [`issue_fields`] leaves out.
#[derive(Debug, Deserialize)]
struct HydratedIssue {
    id: String,
    #[serde(default)]
    assignees: NamedNodes,
    #[serde(default)]
    labels: LabelNodes,
}

#[derive(Debug, Default, Deserialize)]
struct LabelNodes {
    nodes: Vec<Label>,
}

/// The issue a mutation payload reports: scalars plus the two connections
/// the bulk query hydrates separately. On a single node they cost nothing
/// extra, so a mutation can hand back the complete issue (#196).
#[derive(Debug, Deserialize)]
struct MutatedIssue {
    #[serde(flatten)]
    node: IssueNode,
    #[serde(default)]
    assignees: NamedNodes,
    #[serde(default)]
    labels: LabelNodes,
}

impl MutatedIssue {
    fn into_issue(self) -> Issue {
        let mut issue = self.node.to_issue();
        issue.assignees = self.assignees.nodes.into_iter().map(|n| n.login).collect();
        issue.labels = self.labels.nodes;
        issue
    }
}

/// Longest error text shown for a project problem (#253, #254).
const REASON_MAX: usize = 60;

/// Why a project call failed, in a few words (#253, #254). GitHub names the
/// scope it needs in its `INSUFFICIENT_SCOPES` message ("requires one of the
/// following scopes: ['project']"), so that case reads as something to fix;
/// anything else shows a short form of the error. Only the *required* list is
/// searched: the same message also lists the scopes the token does have.
fn scope_reason(err: &ProviderError, scope: &str) -> String {
    let text = err.to_string();
    if let Some((_, after)) = text.split_once("following scopes: [")
        && let Some((required, _)) = after.split_once(']')
        && required.contains(&format!("'{scope}'"))
    {
        return format!("token lacks {scope}");
    }
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= REASON_MAX {
        flat
    } else {
        let cut: String = flat.chars().take(REASON_MAX - 1).collect();
        format!("{cut}…")
    }
}

/// Parse `data.<field>.issue` from a mutation response into an [`Issue`].
fn issue_from_payload(data: &Value, field: &str) -> Result<Issue> {
    let node = data
        .get(field)
        .and_then(|payload| payload.get("issue"))
        .filter(|issue| !issue.is_null())
        .ok_or_else(|| ProviderError::Shape(format!("{field} returned no issue")))?;
    serde_json::from_value::<MutatedIssue>(node.clone())
        .map(MutatedIssue::into_issue)
        .map_err(|e| ProviderError::Shape(e.to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountNode {
    total_count: u64,
}

/// Scalar issue fields only — **no nested connections**.
///
/// A connection nested inside `issues` is charged per issue slot requested:
/// measured against `pgmac-net`, `cost = 1 + n_nested × (reposFirst ×
/// issuesFirst) / 100`, which made the old `assignees`+`labels` inline form
/// cost 101 points a page (202 a refresh) while this shape costs 1. Labels and
/// assignees are hydrated separately by [`Client::hydrate_issues`]. Adding a
/// connection back here silently restores the 100x cost — `issue_fields_has_no_nested_connections`
/// guards it. See `docs/graphql-api-cost.md`.
macro_rules! issue_fields {
    () => {
        concat!(
            "id number title body state url
             author { login }
             comments { totalCount }
             createdAt updatedAt closedAt
             subIssuesSummary { completed total }
             issueDependenciesSummary { blockedBy blocking }
             relatesTo { totalCount }",
            ancestor_fields!()
        )
    };
}

/// What a context row needs of an ancestor it may never load (#234).
macro_rules! stub_fields {
    () => {
        "id number title state url repository { name owner { login } }"
    };
}

/// The parent chain to GitHub's 8-level sub-issue limit. Nested objects, not
/// connections: measured at no extra cost in the bulk query, at any depth.
macro_rules! ancestor_fields {
    () => {
        parent_chain!(1 2 3 4 5 6 7 8)
    };
}

/// One `parent { … }` level per token, each holding the next.
macro_rules! parent_chain {
    () => {
        ""
    };
    ($level:tt $($rest:tt)*) => {
        concat!(" parent { ", stub_fields!(), parent_chain!($($rest)*), " }")
    };
}

/// Everything [`Issue`] holds, for the single issue a mutation reports back.
/// The connection page sizes match [`ISSUE_HYDRATE_QUERY`], so a patched
/// issue carries the same label/assignee window as a fetched one.
macro_rules! full_issue_fields {
    () => {
        concat!(
            issue_fields!(),
            " assignees(first: 10) { nodes { login } }
              labels(first: 20) { nodes { name color } }"
        )
    };
}

/// [`full_issue_fields`] under the `issue` field of a mutation payload.
macro_rules! mutated_issue_fields {
    () => {
        concat!("issue { ", full_issue_fields!(), " }")
    };
}

const CLOSE_ISSUE_MUTATION: &str = concat!(
    "mutation($id: ID!) { closeIssue(input: {issueId: $id}) { ",
    mutated_issue_fields!(),
    " } }"
);

const REOPEN_ISSUE_MUTATION: &str = concat!(
    "mutation($id: ID!) { reopenIssue(input: {issueId: $id}) { ",
    mutated_issue_fields!(),
    " } }"
);

const TRANSFER_ISSUE_MUTATION: &str = concat!(
    "mutation($id: ID!, $repo: ID!) { transferIssue(input: {issueId: $id, repositoryId: $repo, createLabelsIfMissing: true}) { ",
    mutated_issue_fields!(),
    " } }"
);

const CREATE_ISSUE_MUTATION: &str = concat!(
    "mutation($input: CreateIssueInput!) { createIssue(input: $input) { ",
    mutated_issue_fields!(),
    " } }"
);

const UPDATE_ISSUE_MUTATION: &str = concat!(
    "mutation($input: UpdateIssueInput!) { updateIssue(input: $input) { ",
    mutated_issue_fields!(),
    " } }"
);

const ORG_ISSUES_QUERY: &str = concat!(
    "query($org: String!, $reposFirst: Int!, $issuesFirst: Int!, $repoCursor: String, $filter: IssueFilters) {
       rateLimit { cost }
       repositoryOwner(login: $org) {
         repositories(first: $reposFirst, after: $repoCursor, ownerAffiliations: OWNER, isArchived: false, orderBy: {field: NAME, direction: ASC}) {
           pageInfo { hasNextPage endCursor }
           nodes {
             name url hasIssuesEnabled
             issues(first: $issuesFirst, filterBy: $filter, orderBy: {field: UPDATED_AT, direction: DESC}) {
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
    "query($owner: String!, $name: String!, $issuesFirst: Int!, $issueCursor: String, $filter: IssueFilters) {
       rateLimit { cost }
       repository(owner: $owner, name: $name) {
         issues(first: $issuesFirst, after: $issueCursor, filterBy: $filter, orderBy: {field: UPDATED_AT, direction: DESC}) {
           pageInfo { hasNextPage endCursor }
           nodes { ",
    issue_fields!(),
    " }
         }
       }
     }"
);

/// One repository with its first page of issues (#229): the bulk query's
/// repo node for a single named repo, plus `isArchived`, which the bulk query
/// filters on instead of selecting. One connection, scalars only.
const REPO_QUERY: &str = concat!(
    "query($owner: String!, $name: String!, $issuesFirst: Int!, $filter: IssueFilters) {
       rateLimit { cost }
       repository(owner: $owner, name: $name) {
         name url hasIssuesEnabled isArchived
         issues(first: $issuesFirst, filterBy: $filter, orderBy: {field: UPDATED_AT, direction: DESC}) {
           pageInfo { hasNextPage endCursor }
           nodes { ",
    issue_fields!(),
    " }
         }
       }
     }"
);

/// The repos `ORG_ISSUES_QUERY` walks, names only (#230). No connections
/// inside a repo node, so a page costs 1 point.
const REPO_NAMES_QUERY: &str = "query($org: String!, $cursor: String) {
  rateLimit { cost }
  repositoryOwner(login: $org) {
    repositories(first: 100, after: $cursor, ownerAffiliations: OWNER, isArchived: false, orderBy: {field: NAME, direction: ASC}) {
      pageInfo { hasNextPage endCursor }
      nodes { name hasIssuesEnabled }
    }
  }
}";

/// One issue as it stands, with the same label/assignee window as a hydrated
/// bulk issue (#229). Connections on a single node are not billed per slot.
const ISSUE_NODE_QUERY: &str = concat!(
    "query($id: ID!) { rateLimit { cost } node(id: $id) { ... on Issue { ",
    full_issue_fields!(),
    " } } }"
);

/// Labels and assignees for up to 100 issues at a time. `nodes(ids:)` is a
/// plain list, not a connection, so the two connections inside it are charged
/// once for the batch instead of once per issue slot — 2 points per 100
/// issues, against 100 for the same data nested in the bulk query.
const ISSUE_HYDRATE_QUERY: &str = "
query($ids: [ID!]!) {
  rateLimit { cost }
  nodes(ids: $ids) {
    ... on Issue {
      id
      assignees(first: 10) { nodes { login } }
      labels(first: 20) { nodes { name color } }
    }
  }
}";

const COMMENTS_QUERY: &str = "
query($id: ID!) {
  node(id: $id) {
    ... on Issue {
      comments(first: 100) {
        nodes { id author { login } createdAt body }
      }
    }
  }
}";

const PR_SUMMARY_QUERY: &str = "
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issueOrPullRequest(number: $number) {
      __typename
      ... on PullRequest {
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
                    ... on CheckRun { name conclusion detailsUrl }
                    ... on StatusContext { context state targetUrl }
                  }
                }
              }
              checkSuites(first: 10) { nodes { ...CheckSuiteFields } }
            }
          }
        }
      }
      ... on Issue {
        number
        title
        url
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
  workflowRun { runNumber event url workflow { name } }
}";

// ---- pull_request response DTOs ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrRepoResponse {
    /// One number, resolved to whichever kind it names. **Not** two sibling
    /// `pullRequest`/`issue` fields: GitHub answers a wrong-type lookup with a
    /// `NOT_FOUND` entry in `errors`, not a null, and `graphql_data` rejects
    /// any response carrying errors — so a two-field query fails for *every*
    /// number, whichever kind it is. The union asks once and is told the type.
    issue_or_pull_request: Option<IssueOrPr>,
    default_branch_ref: Option<DefaultBranchRefNode>,
}

/// The `IssueOrPullRequest` union, discriminated by GraphQL's `__typename`.
#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum IssueOrPr {
    PullRequest(PullRequestNode),
    Issue(PrIssueNode),
}

#[derive(Debug, Deserialize)]
struct PrIssueNode {
    number: u64,
    title: String,
    url: String,
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
#[serde(rename_all = "camelCase")]
struct ContextNode {
    #[serde(rename = "__typename")]
    typename: String,
    name: Option<String>,
    conclusion: Option<String>,
    context: Option<String>,
    state: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
}

impl ContextNode {
    fn into_info(self) -> Option<CheckContextInfo> {
        match self.typename.as_str() {
            "CheckRun" => Some(CheckContextInfo {
                name: self.name.unwrap_or_default(),
                conclusion: self.conclusion.unwrap_or_else(|| "PENDING".into()),
                url: self.details_url.unwrap_or_default(),
            }),
            "StatusContext" => Some(CheckContextInfo {
                name: self.context.unwrap_or_default(),
                conclusion: self.state.unwrap_or_default(),
                url: self.target_url.unwrap_or_default(),
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
            url: wr.url,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRunNode {
    run_number: u64,
    event: String,
    url: String,
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

/// Resolve one reference. A [`PrRef`] parsed from an issue thread is only a
/// candidate — the shorthand forms (`o/r#N`, `#N`) cannot distinguish a pull
/// request from an issue, because GitHub numbers both from one per-repo
/// sequence. Landing on the issue is therefore a normal outcome, not a failure.
///
/// A number that names neither never reaches here: GitHub reports it as a
/// `NOT_FOUND` error, which `graphql_data` turns into `Err` before the response
/// is mapped. The `None` arm is defensive only.
fn map_pr_lookup(pr: PrRef, data: PrRepoResponse) -> Result<PrLookup> {
    let node = match data.issue_or_pull_request {
        Some(IssueOrPr::PullRequest(n)) => n,
        Some(IssueOrPr::Issue(i)) => {
            return Ok(PrLookup::Issue(IssueRef {
                owner: pr.owner,
                repo: pr.repo,
                number: i.number,
                title: i.title,
                url: i.url,
            }));
        }
        None => return Err(ProviderError::Shape(format!("no such PR {}", pr.label()))),
    };

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

    Ok(PrLookup::Pr(Box::new(PrSummary {
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
    })))
}

/// Thin delegation to the inherent methods above, which keep their own
/// unit tests. Inherent methods win name resolution inside this impl, so
/// each call goes to the real implementation, not back into the trait.
#[async_trait::async_trait]
impl crate::provider::IssueProvider for Client {
    async fn org_issues(&self, org: &str, scope: &FetchScope) -> Result<Vec<RepoIssues>> {
        self.org_issues(org, scope).await
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
        self.repo_issues(org, repo, scope).await
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

    async fn create_issue(&self, p: &NewIssueParams) -> Result<Created> {
        self.create_issue(p).await
    }

    fn rate_limit(&self) -> Option<RateLimitData> {
        self.rate_limit()
    }

    fn host(&self) -> String {
        "github.com".into()
    }

    fn supports_pr_summary(&self) -> bool {
        true
    }

    async fn pull_request(&self, pr: &PrRef) -> Result<PrLookup> {
        self.pull_request(pr).await
    }

    fn supports_runs(&self) -> bool {
        true
    }

    async fn linked_prs(&self, issue_id: &str, text_refs: &[PrRef]) -> Result<Vec<LinkedPr>> {
        self.linked_prs(issue_id, text_refs).await
    }

    async fn head_runs(&self, owner: &str, repo: &str, sha: &str) -> Result<Vec<ActionsRun>> {
        self.head_runs(owner, repo, sha).await
    }

    async fn run_jobs(&self, owner: &str, repo: &str, run_id: i64) -> Result<Vec<ActionsJob>> {
        self.run_jobs(owner, repo, run_id).await
    }

    async fn history_runs(
        &self,
        owner: &str,
        repo: &str,
        workflow_id: i64,
        event: &str,
    ) -> Result<Vec<ActionsRun>> {
        self.history_runs(owner, repo, workflow_id, event).await
    }

    fn runs_rate_limit(&self) -> Option<RateLimitData> {
        self.rest_rate_limit.get()
    }

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
        assert_eq!(issue.comment_count, 3);
        assert!(issue.closed_at.is_none());
        // Labels and assignees are not part of the bulk query's shape — they
        // arrive from `hydrate_issues`, so the node leaves them empty even
        // when the payload happens to carry them.
        assert!(issue.assignees.is_empty());
        assert!(issue.labels.is_empty());
    }

    #[test]
    fn hydrated_issue_carries_labels_and_assignees() {
        let raw = serde_json::json!({
            "id": "I_abc",
            "assignees": {"nodes": [{"login": "pgmac"}]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}]}
        });
        let h: HydratedIssue = serde_json::from_value(raw).unwrap();
        assert_eq!(h.id, "I_abc");
        assert_eq!(h.assignees.nodes[0].login, "pgmac");
        assert_eq!(h.labels.nodes[0].name, "bug");
    }

    #[test]
    fn hydrated_issue_tolerates_missing_connections() {
        // An id the viewer can no longer resolve comes back as a bare object.
        let h: HydratedIssue = serde_json::from_value(serde_json::json!({"id": "I_x"})).unwrap();
        assert!(h.assignees.nodes.is_empty());
        assert!(h.labels.nodes.is_empty());
    }

    #[test]
    fn hydrate_batch_matches_githubs_node_id_limit() {
        assert_eq!(HYDRATE_BATCH, 100);
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
    fn resource_limited_graphql_error_detected_by_type() {
        let errors = serde_json::json!([
            {"type": "EXCESSIVE_RESOURCE_USAGE", "message": "Resource limits for this query exceeded."}
        ]);
        assert!(errors_contain_resource_limited(&errors));
    }

    #[test]
    fn resource_limited_graphql_error_detected_by_message_when_untyped() {
        let errors = serde_json::json!([
            {"message": "Resource limits for this query exceeded."}
        ]);
        assert!(errors_contain_resource_limited(&errors));

        let unrelated = serde_json::json!([{"message": "field X doesn't exist"}]);
        assert!(!errors_contain_resource_limited(&unrelated));
    }

    #[test]
    fn join_error_messages_concatenates_all_entries() {
        let errors = serde_json::json!([
            {"message": "first problem"},
            {"message": "second problem"}
        ]);
        assert_eq!(
            join_error_messages(&errors),
            "first problem; second problem"
        );
    }

    #[test]
    fn join_error_messages_falls_back_to_raw_json_when_no_messages() {
        let errors = serde_json::json!([{"type": "SOMETHING"}]);
        assert_eq!(join_error_messages(&errors), errors.to_string());
    }

    #[test]
    fn page_sizes_shrink_halves_toward_floor_then_stops() {
        let mut sizes = PageSizes {
            repos: REPOS_PAGE,
            issues: ISSUES_PAGE,
        };
        assert!(sizes.shrink());
        assert_eq!(sizes.repos, 50);
        assert_eq!(sizes.issues, 50);

        assert!(sizes.shrink());
        assert_eq!(sizes.repos, 25);
        assert_eq!(sizes.issues, 25);

        assert!(sizes.shrink());
        assert_eq!(sizes.repos, 12);
        assert_eq!(sizes.issues, 12);

        assert!(sizes.shrink());
        assert_eq!(sizes.repos, 6);
        assert_eq!(sizes.issues, MIN_ISSUES_PAGE);

        assert!(sizes.shrink());
        assert_eq!(sizes.repos, MIN_REPOS_PAGE);
        assert_eq!(sizes.issues, MIN_ISSUES_PAGE);

        // Already at floor — no further shrink possible.
        assert!(!sizes.shrink());
        assert_eq!(sizes.repos, MIN_REPOS_PAGE);
        assert_eq!(sizes.issues, MIN_ISSUES_PAGE);
    }

    #[test]
    fn queries_request_all_issue_fields() {
        for field in ["createdAt", "updatedAt", "closedAt", "totalCount"] {
            assert!(ORG_ISSUES_QUERY.contains(field));
            assert!(REPO_ISSUES_QUERY.contains(field));
        }
    }

    /// The cost guard for #107.
    ///
    /// A connection nested inside `issues` is charged per issue slot
    /// requested, so re-inlining `labels`/`assignees` here takes a full org
    /// fetch from 1 point back to 101 per page — with no visible symptom until
    /// the hourly budget is gone. Hydration exists precisely so these two stay
    /// out; if this test is in your way, read `docs/graphql-api-cost.md` before
    /// changing it.
    #[test]
    fn issue_fields_has_no_nested_connections() {
        let fields = issue_fields!();
        for banned in ["assignees", "labels", "projectItems", "timelineItems"] {
            assert!(
                !fields.contains(banned),
                "`{banned}` is a nested connection inside `issues` — it costs \
                 (reposFirst × issuesFirst) / 100 points per fetch. Hydrate it \
                 via `nodes(ids:)` instead. See docs/graphql-api-cost.md"
            );
        }
        // `first:` anywhere in the issue fields means some connection crept
        // back in, whatever it is called.
        assert!(
            !fields.contains("first:"),
            "no connection may be paginated inside `issues` — see docs/graphql-api-cost.md"
        );
    }

    /// #234: the ancestor chain reaches GitHub's 8-level sub-issue limit.
    #[test]
    fn issue_fields_carry_an_eight_level_parent_chain() {
        let fields = issue_fields!();
        assert_eq!(fields.matches("parent {").count(), 8, "{fields}");
        assert!(fields.contains("subIssuesSummary { completed total }"));
        assert!(fields.contains("issueDependenciesSummary { blockedBy blocking }"));
        assert!(
            fields.contains("relatesTo { totalCount }"),
            "a count with no `first:` is not billed per slot"
        );
    }

    #[test]
    fn a_parent_chain_flattens_nearest_first() {
        let node = serde_json::json!({
            "id": "I_3", "number": 3, "title": "grandchild", "body": "", "state": "OPEN",
            "url": "u", "author": {"login": "a"}, "comments": {"totalCount": 0},
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
            "closedAt": null,
            "subIssuesSummary": {"completed": 0, "total": 0},
            "issueDependenciesSummary": {"blockedBy": 1, "blocking": 2},
            "relatesTo": {"totalCount": 3},
            "parent": {
                "id": "I_2", "number": 2, "title": "child", "state": "OPEN", "url": "u2",
                "repository": {"name": "r", "owner": {"login": "o"}},
                "parent": {
                    "id": "I_1", "number": 1, "title": "root", "state": "CLOSED", "url": "u1",
                    "repository": {"name": "other", "owner": {"login": "o"}},
                    "parent": null
                }
            }
        });
        let issue = serde_json::from_value::<IssueNode>(node)
            .unwrap()
            .to_issue();
        let numbers: Vec<_> = issue.ancestors.iter().map(|a| a.number).collect();
        assert_eq!(numbers, vec![2, 1]);
        assert_eq!(issue.ancestors[1].repo, "other");
        assert_eq!(issue.ancestors[1].state, IssueState::Closed);
        assert_eq!(issue.sub_issues, None, "no sub-issues reads as none");
        assert_eq!(
            issue.relations,
            Some(Relations {
                blocked_by: 1,
                blocking: 2,
                related: 3
            })
        );
    }

    #[test]
    fn a_top_level_parent_reports_its_progress() {
        let node = serde_json::json!({
            "id": "I_1", "number": 227, "title": "t", "body": "", "state": "OPEN",
            "url": "u", "author": null, "comments": {"totalCount": 0},
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
            "closedAt": null, "parent": null,
            "subIssuesSummary": {"completed": 4, "total": 5},
            "issueDependenciesSummary": {"blockedBy": 0, "blocking": 0},
            "relatesTo": {"totalCount": 0}
        });
        let issue = serde_json::from_value::<IssueNode>(node)
            .unwrap()
            .to_issue();
        assert!(issue.ancestors.is_empty());
        assert_eq!(
            issue.sub_issues,
            Some(SubIssueProgress {
                completed: 4,
                total: 5
            })
        );
        assert_eq!(issue.relations, None, "all-zero relations read as none");
    }

    /// #196: every issue-level mutation reports the full issue back —
    /// labels and assignees included, since on a single node they cost
    /// nothing extra — so the list can be patched without a refetch.
    #[test]
    fn issue_mutations_report_the_full_issue() {
        for m in [
            CLOSE_ISSUE_MUTATION,
            REOPEN_ISSUE_MUTATION,
            UPDATE_ISSUE_MUTATION,
            // #226: a move is placed from its answer, not an org refetch.
            TRANSFER_ISSUE_MUTATION,
            // #249: so is a create.
            CREATE_ISSUE_MUTATION,
        ] {
            for field in [
                "issue {",
                "updatedAt",
                "closedAt",
                "assignees(first: 10)",
                "labels(first: 20)",
            ] {
                assert!(m.contains(field), "{field} missing from {m}");
            }
            assert!(!m.contains("clientMutationId"));
        }
    }

    #[test]
    fn mutation_payload_maps_to_the_updated_issue() {
        let data = json!({ "closeIssue": { "issue": {
            "id": "I_1", "number": 196, "title": "Tree view updates",
            "body": "b", "state": "CLOSED", "url": "u",
            "author": { "login": "pgmac" },
            "comments": { "totalCount": 3 },
            "createdAt": "2026-10-01T00:00:00Z",
            "updatedAt": "2026-10-16T10:00:00Z",
            "closedAt": "2026-10-16T10:00:00Z",
            "assignees": { "nodes": [{ "login": "pgmac" }] },
            "labels": { "nodes": [{ "name": "bug", "color": "d73a4a" }] }
        }}});
        let issue = issue_from_payload(&data, "closeIssue").unwrap();
        assert_eq!(issue.id, "I_1");
        assert_eq!(issue.state, IssueState::Closed);
        assert_eq!(issue.comment_count, 3);
        assert_eq!(issue.assignees, vec!["pgmac".to_string()]);
        assert_eq!(issue.labels[0].name, "bug");
        assert!(issue.closed_at.is_some());
    }

    /// #253: the project list is its own query. A token without `read:project`
    /// fails any query that names `projectsV2`, so it must not share the one
    /// that carries the repo id the form cannot do without.
    #[test]
    fn projects_are_not_in_the_main_options_query() {
        let src = include_str!("client.rs");
        let start = src.find("pub async fn repo_form_options").unwrap();
        let end = src[start..].find("async fn repo_projects").unwrap();
        let body = &src[start..start + end];
        // The only mention left in that function is the call to the helper.
        assert!(
            !body.contains("projectsV2("),
            "projectsV2 is back in the main query"
        );
        assert!(body.contains("repo_projects"));
    }

    #[test]
    fn a_missing_project_scope_reads_as_something_to_fix() {
        let e = ProviderError::Api(
            "Your token has not been granted the required scopes to execute this query. \
             The 'projectsV2' field requires one of the following scopes: ['read:project'], \
             but your token has only been granted the: ['repo'] scopes."
                .into(),
        );
        assert_eq!(scope_reason(&e, "read:project"), "token lacks read:project");
    }

    /// #254: the add-to-project mutation needs `project`. The message also
    /// lists the scopes the token *has*, which can include `read:project`.
    #[test]
    fn a_missing_write_scope_is_matched_in_the_required_list_only() {
        let e = ProviderError::Api(
            "Your token has not been granted the required scopes to execute this query. \
             The 'addProjectV2ItemById' field requires one of the following scopes: ['project'], \
             but your token has only been granted the: ['read:project', 'repo'] scopes."
                .into(),
        );
        assert_eq!(scope_reason(&e, "project"), "token lacks project");
        // The granted list names read:project; that must not read as the cause.
        assert_ne!(scope_reason(&e, "read:project"), "token lacks read:project");
    }

    #[test]
    fn any_other_projects_error_shows_a_short_form_of_it() {
        let short = ProviderError::Api("boom".into());
        assert_eq!(scope_reason(&short, "project"), "API errors: boom");

        let long = ProviderError::Api("x ".repeat(100));
        let why = scope_reason(&long, "project");
        assert!(why.chars().count() <= REASON_MAX, "{why}");
        assert!(why.ends_with('…'));
    }

    #[test]
    fn create_payload_maps_to_the_created_issue() {
        let data = json!({ "createIssue": { "issue": {
            "id": "I_9", "number": 249, "title": "Slow to display",
            "body": "b", "state": "OPEN", "url": "https://github.com/o/r/issues/249",
            "author": { "login": "pgmac" },
            "comments": { "totalCount": 0 },
            "createdAt": "2026-11-04T00:00:00Z",
            "updatedAt": "2026-11-04T00:00:00Z",
            "closedAt": null,
            "assignees": { "nodes": [] },
            "labels": { "nodes": [{ "name": "bug", "color": "d73a4a" }] }
        }}});
        let issue = issue_from_payload(&data, "createIssue").unwrap();
        assert_eq!(issue.id, "I_9");
        assert_eq!(issue.number, 249);
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.labels[0].name, "bug");
    }

    #[test]
    fn mutation_payload_without_an_issue_is_a_shape_error() {
        for data in [
            json!({ "updateIssue": { "issue": null } }),
            json!({ "updateIssue": {} }),
            json!({}),
        ] {
            assert!(matches!(
                issue_from_payload(&data, "updateIssue"),
                Err(ProviderError::Shape(_))
            ));
        }
    }

    #[test]
    fn hydrate_query_carries_the_connections_issue_fields_drops() {
        assert!(ISSUE_HYDRATE_QUERY.contains("assignees(first: 10)"));
        assert!(ISSUE_HYDRATE_QUERY.contains("labels(first: 20)"));
        assert!(ISSUE_HYDRATE_QUERY.contains("nodes(ids: $ids)"));
    }

    #[test]
    fn bulk_queries_report_their_cost() {
        for query in [
            ORG_ISSUES_QUERY,
            REPO_ISSUES_QUERY,
            ISSUE_HYDRATE_QUERY,
            REPO_QUERY,
            ISSUE_NODE_QUERY,
        ] {
            assert!(query.contains("rateLimit { cost }"));
        }
    }

    /// #229: the payload GitHub returned live for a deleted node / missing repo.
    #[test]
    fn not_found_is_only_an_all_not_found_error_list() {
        let gone = serde_json::json!([{"type": "NOT_FOUND", "path": ["node"],
            "message": "Could not resolve to a node with the global id of 'I_x'."}]);
        assert!(errors_all_not_found(&gone));
        let mixed = serde_json::json!([
            {"type": "NOT_FOUND", "message": "a"},
            {"type": "FORBIDDEN", "message": "b"}
        ]);
        assert!(!errors_all_not_found(&mixed));
        assert!(!errors_all_not_found(&serde_json::json!([])));
    }

    /// The single-repo query mirrors the bulk repo node, with `isArchived`
    /// selected because there is no `isArchived: false` argument to filter on.
    #[test]
    fn repo_query_selects_what_the_bulk_query_filters_on() {
        assert!(REPO_QUERY.contains("hasIssuesEnabled isArchived"));
        assert!(REPO_QUERY.contains("filterBy: $filter"));
        assert!(REPO_QUERY.contains(issue_fields!()));
    }

    #[test]
    fn single_issue_read_carries_what_hydration_adds() {
        assert!(ISSUE_NODE_QUERY.contains("node(id: $id)"));
        assert!(ISSUE_NODE_QUERY.contains("assignees(first: 10)"));
        assert!(ISSUE_NODE_QUERY.contains("labels(first: 20)"));
    }

    #[test]
    fn single_issue_node_parses_like_a_mutation_payload() {
        let node = serde_json::json!({
            "id": "I_1", "number": 7, "title": "t", "body": "", "state": "OPEN",
            "url": "u", "author": {"login": "a"}, "comments": {"totalCount": 2},
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
            "closedAt": null,
            "assignees": {"nodes": [{"login": "pgmac"}]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}]}
        });
        let issue = serde_json::from_value::<MutatedIssue>(node)
            .unwrap()
            .into_issue();
        assert_eq!(issue.number, 7);
        assert_eq!(issue.assignees, vec!["pgmac"]);
        assert_eq!(issue.labels[0].name, "bug");
    }

    #[test]
    fn an_empty_scope_filters_on_state_only() {
        assert_eq!(
            issue_filter(&FetchScope::default()),
            serde_json::json!({ "states": ["OPEN"] })
        );
    }

    #[test]
    fn a_full_scope_maps_to_issue_filters() {
        let scope = FetchScope {
            include_closed: true,
            assignee: Some("pgmac".into()),
            author: Some("someone".into()),
            updated_since: Some("2026-10-01T00:00:00Z".parse().unwrap()),
            labels: vec!["high".into(), "priority:high".into()],
        };
        assert_eq!(
            issue_filter(&scope),
            serde_json::json!({
                "states": ["OPEN", "CLOSED"],
                "assignee": "pgmac",
                "createdBy": "someone",
                "since": "2026-10-01T00:00:00+00:00",
                "labels": ["high", "priority:high"],
            })
        );
    }

    /// The live error for `filterBy: {assignee: "<no such login>"}`.
    #[test]
    fn only_the_unknown_assignee_error_triggers_the_fallback() {
        let unknown = ProviderError::Api(
            "Could not find an assignee with the login 'no-such-user-zz9q'.".into(),
        );
        assert!(is_unknown_assignee(&unknown));
        assert!(!is_unknown_assignee(&ProviderError::Api("boom".into())));
        assert!(!is_unknown_assignee(&ProviderError::NotFound(
            "Could not find an assignee".into()
        )));
        let fallback = without_assignee(&FetchScope {
            assignee: Some("x".into()),
            labels: vec!["l".into()],
            ..FetchScope::default()
        });
        assert_eq!(fallback.assignee, None);
        assert_eq!(fallback.labels, vec!["l"]);
    }

    #[test]
    fn issue_queries_filter_through_filter_by() {
        for query in [ORG_ISSUES_QUERY, REPO_ISSUES_QUERY, REPO_QUERY] {
            assert!(query.contains("$filter: IssueFilters"), "{query}");
            assert!(query.contains("filterBy: $filter"), "{query}");
            assert!(!query.contains("states: $states"), "{query}");
        }
    }

    #[test]
    fn repo_names_query_walks_the_same_repos_without_issues() {
        assert!(REPO_NAMES_QUERY.contains("ownerAffiliations: OWNER, isArchived: false"));
        assert!(REPO_NAMES_QUERY.contains("nodes { name hasIssuesEnabled }"));
        assert!(!REPO_NAMES_QUERY.contains("issues("));
    }

    #[test]
    fn issue_states_widen_with_include_closed() {
        assert_eq!(issue_states(false), vec!["OPEN"]);
        assert_eq!(issue_states(true), vec!["OPEN", "CLOSED"]);
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

    /// Unwrap the PR case; panics on the issue case, which these tests never
    /// exercise (see `pr_lookup_*` below for that side).
    fn expect_pr(lookup: PrLookup) -> PrSummary {
        match lookup {
            PrLookup::Pr(s) => *s,
            PrLookup::Issue(i) => panic!("expected a PR, got issue {}", i.label()),
        }
    }

    #[test]
    fn pr_summary_parses_mixed_check_union_and_reviews() {
        let raw = serde_json::json!({
            "issueOrPullRequest": {
                "__typename": "PullRequest",
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
                                        {"__typename": "CheckRun", "name": "ci", "conclusion": "SUCCESS", "context": null, "state": null, "detailsUrl": "https://github.com/o/r/runs/1"},
                                        {"__typename": "StatusContext", "name": null, "conclusion": null, "context": "legacy-ci", "state": "SUCCESS", "targetUrl": "https://ci.example.com/legacy"}
                                    ]
                                }
                            },
                            "checkSuites": {
                                "nodes": [{
                                    "conclusion": "SUCCESS",
                                    "createdAt": "2026-07-20T00:00:00Z",
                                    "workflowRun": {"runNumber": 42, "event": "pull_request", "url": "https://github.com/o/r/actions/runs/42", "workflow": {"name": "ci.yml"}}
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
                                    "workflowRun": {"runNumber": 128, "event": "push", "url": "https://github.com/o/r/actions/runs/128", "workflow": {"name": "release.yml"}}
                                }]
                            }
                        }]
                    }
                }
            }
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        let summary = expect_pr(map_pr_lookup(sample_pr_ref(), repo).unwrap());

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
        assert_eq!(checks.contexts[0].url, "https://github.com/o/r/runs/1");
        assert_eq!(checks.contexts[1].name, "legacy-ci");
        assert_eq!(checks.contexts[1].url, "https://ci.example.com/legacy");

        assert_eq!(summary.pr_runs.len(), 1);
        assert_eq!(summary.pr_runs[0].workflow, "ci.yml");
        assert_eq!(
            summary.pr_runs[0].url,
            "https://github.com/o/r/actions/runs/42"
        );
        assert_eq!(summary.default_branch_name, "main");
        assert_eq!(summary.default_branch_runs.len(), 1);
        assert_eq!(summary.default_branch_runs[0].workflow, "release.yml");
        assert_eq!(
            summary.default_branch_runs[0].url,
            "https://github.com/o/r/actions/runs/128"
        );
    }

    #[test]
    fn pr_summary_handles_empty_rollup_and_no_default_branch() {
        let raw = serde_json::json!({
            "issueOrPullRequest": {
                "__typename": "PullRequest",
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
        let summary = expect_pr(map_pr_lookup(sample_pr_ref(), repo).unwrap());

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
            "detailsUrl",
            "targetUrl",
        ] {
            assert!(PR_SUMMARY_QUERY.contains(field), "missing {field}");
        }
    }

    /// #129 must resolve the number through the **union**, not through sibling
    /// `pullRequest`/`issue` fields.
    ///
    /// This is the regression that shipped: GitHub answers a wrong-type lookup
    /// with a `NOT_FOUND` entry in `errors` rather than a null, and
    /// `graphql_data` rejects any response carrying errors. So a two-field
    /// query fails for *every* number — a PR errors on `issue`, an issue errors
    /// on `pullRequest` — and no mapping code ever runs. Nothing else in the
    /// suite catches it, because the fixtures feed `map_pr_lookup` directly and
    /// never see the transport.
    #[test]
    fn pr_summary_query_resolves_the_number_through_the_union() {
        assert!(
            PR_SUMMARY_QUERY.contains("issueOrPullRequest(number: $number)"),
            "the union is what lets one number be resolved without an error"
        );
        assert!(
            !PR_SUMMARY_QUERY.contains("pullRequest(number: $number)"),
            "a sibling pullRequest field errors for every issue number"
        );
        assert!(
            !PR_SUMMARY_QUERY.contains("issue(number: $number)"),
            "a sibling issue field errors for every PR number"
        );
    }

    /// #129: a shorthand like `o/r#72` cannot say whether 72 is a PR or an
    /// issue, so resolving it to an issue is a normal outcome.
    #[test]
    fn pr_lookup_reports_an_issue_when_the_number_is_not_a_pr() {
        let raw = serde_json::json!({
            "issueOrPullRequest": {
                "__typename": "Issue",
                "number": 72,
                "title": "Not a PR at all",
                "url": "https://github.com/pgmac-net/gh-issues-tui/issues/72"
            },
            "defaultBranchRef": null
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        let lookup = map_pr_lookup(sample_pr_ref(), repo).unwrap();

        let PrLookup::Issue(i) = lookup else {
            panic!("expected the issue case");
        };
        assert_eq!(i.label(), "pgmac-net/gh-issues-tui#72");
        assert_eq!(i.title, "Not a PR at all");
        assert_eq!(
            i.url,
            "https://github.com/pgmac-net/gh-issues-tui/issues/72"
        );
    }

    /// A PR number must still map to the PR case — the half of the union that
    /// the shipped bug broke in the field while every fixture still passed.
    #[test]
    fn pr_lookup_reports_a_pr_when_the_number_is_a_pr() {
        let raw = serde_json::json!({
            "issueOrPullRequest": {
                "__typename": "PullRequest",
                "title": "Real PR",
                "body": "",
                "state": "OPEN",
                "isDraft": false,
                "baseRefName": "main",
                "headRefName": "feature",
                "additions": 1,
                "deletions": 0,
                "changedFiles": 1,
                "comments": {"totalCount": 0},
                "reviewThreads": {"totalCount": 0},
                "reviewDecision": null,
                "reviews": {"nodes": []},
                "commits": {"nodes": []}
            },
            "defaultBranchRef": null
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(
            expect_pr(map_pr_lookup(sample_pr_ref(), repo).unwrap()).title,
            "Real PR"
        );
    }

    /// Defensive only — in practice GitHub reports an unknown number as an
    /// `errors` entry, which fails the request before mapping.
    #[test]
    fn pr_lookup_errors_when_the_number_is_neither() {
        let raw = serde_json::json!({
            "issueOrPullRequest": null,
            "defaultBranchRef": null
        });
        let repo: PrRepoResponse = serde_json::from_value(raw).unwrap();
        let err = map_pr_lookup(sample_pr_ref(), repo).unwrap_err();
        assert!(
            err.to_string()
                .contains("no such PR pgmac-net/gh-issues-tui#72"),
            "unexpected error: {err}"
        );
    }

    // ---- Characterisation goldens (issue #87) ---------------------------
    //
    // Rate-limit header parsing, the rate-limit message format, and
    // `parse_at` all move to the shared provider layer in the dedup phase.
    // They had no direct coverage, so pin them here first.

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
    fn rate_limit_headers_are_read_in_seconds() {
        let c = Client::new("t".into()).unwrap();
        assert!(c.rate_limit().is_none(), "starts unset");

        c.update_rate_limit(&headers(&[
            ("x-ratelimit-remaining", "4321"),
            ("x-ratelimit-limit", "5000"),
            // GitHub reports the reset as epoch *seconds*, stored as-is.
            ("x-ratelimit-reset", "1800000000"),
        ]));

        let rl = c.rate_limit().expect("rate limit recorded");
        assert_eq!(rl.remaining, 4321);
        assert_eq!(rl.limit, 5000);
        assert_eq!(rl.reset, 1_800_000_000);
    }

    #[test]
    fn rate_limit_ignores_partial_header_sets() {
        let c = Client::new("t".into()).unwrap();
        // All three must be present; a partial set leaves the state alone.
        c.update_rate_limit(&headers(&[("x-ratelimit-remaining", "1")]));
        assert!(c.rate_limit().is_none());

        c.update_rate_limit(&headers(&[
            ("x-ratelimit-remaining", "not-a-number"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", "1800000000"),
        ]));
        assert!(c.rate_limit().is_none(), "unparseable value is ignored");
    }

    #[test]
    fn rate_limit_message_carries_the_shared_prefix_and_counts() {
        use crate::provider::error::RATE_LIMIT_MSG_PREFIX;

        let c = Client::new("t".into()).unwrap();
        c.update_rate_limit(&headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", "1800000000"),
        ]));

        let msg = c.rate_limit.message(None);
        assert!(
            msg.starts_with(RATE_LIMIT_MSG_PREFIX),
            "event loop classifies by this prefix: {msg}"
        );
        assert!(msg.contains("0/5000 used"), "{msg}");
        assert!(msg.contains(&c.rate_limit().unwrap().reset_time()), "{msg}");
    }

    #[test]
    fn parse_at_walks_a_nested_path() {
        let data = serde_json::json!({"a": {"b": {"c": 41}}});
        let got: u64 = parse_at(&data, &["a", "b", "c"]).unwrap();
        assert_eq!(got, 41);
    }

    #[test]
    fn parse_at_reports_the_full_path_when_a_segment_is_missing() {
        let data = serde_json::json!({"a": {"b": {}}});
        let err = parse_at::<u64>(&data, &["a", "b", "c"]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("a.b.c"), "error should name the path: {msg}");
        assert!(matches!(err, ProviderError::Shape(_)));
    }

    #[test]
    fn parse_at_reports_a_shape_error_on_type_mismatch() {
        let data = serde_json::json!({"a": "not a number"});
        let err = parse_at::<u64>(&data, &["a"]).unwrap_err();
        assert!(matches!(err, ProviderError::Shape(_)));
    }
}
