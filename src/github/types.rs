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
