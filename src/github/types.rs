use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum IssueState {
    Open,
    Closed,
}

impl std::fmt::Display for IssueState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueState::Open => write!(f, "open"),
            IssueState::Closed => write!(f, "closed"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    /// GraphQL node id, needed for mutations.
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub url: String,
    pub author: String,
    pub assignees: Vec<String>,
    pub labels: Vec<Label>,
    pub comment_count: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct RepoIssues {
    pub repo: String,
    pub repo_url: String,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoLabel {
    pub id: String,
    pub name: String,
}

/// A GraphQL node id + display name, as shown in new-issue form pickers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IdName {
    pub id: String,
    pub name: String,
}

/// Everything the new-issue form's pickers need, fetched per repo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormOptions {
    pub repo_id: String,
    pub labels: Vec<IdName>,
    /// Assignable users; `name` is the login.
    pub users: Vec<IdName>,
    /// Open milestones; `name` is the title.
    pub milestones: Vec<IdName>,
    /// ProjectsV2 linked to the repo; `name` is the title.
    pub projects: Vec<IdName>,
    /// Issue types (org feature; empty when unavailable).
    pub issue_types: Vec<IdName>,
}

/// Parameters for `Client::create_issue`. Ids come from `FormOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewIssueParams {
    pub repo_id: String,
    pub title: String,
    pub body: String,
    pub assignee_ids: Vec<String>,
    pub label_ids: Vec<String>,
    pub milestone_id: Option<String>,
    pub issue_type_id: Option<String>,
    /// Applied after creation via `addProjectV2ItemById`.
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimitData {
    pub remaining: u64,
    pub limit: u64,
    pub reset: i64,
}

impl RateLimitData {
    pub fn reset_time(&self) -> String {
        match chrono::DateTime::from_timestamp(self.reset, 0) {
            Some(dt) => {
                let now = chrono::Utc::now().timestamp();
                let diff = self.reset - now;
                if diff > 60 {
                    format!("{} (in {}m)", dt.format("%H:%M UTC"), diff / 60)
                } else if diff > 0 {
                    format!("{} (in {}s)", dt.format("%H:%M UTC"), diff)
                } else {
                    dt.format("%H:%M UTC").to_string()
                }
            }
            None => format!("epoch {}", self.reset),
        }
    }
}
