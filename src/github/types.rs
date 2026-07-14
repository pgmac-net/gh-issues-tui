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

/// The value part of a `priority:<value>` label name, `None` for other labels.
pub fn priority_value(name: &str) -> Option<&str> {
    let (prefix, value) = name.split_at_checked("priority:".len())?;
    prefix.eq_ignore_ascii_case("priority:").then_some(value)
}

/// Rank of a known priority value: low = 1, medium = 2, high = 3, urgent = 4.
/// `None` for anything else — callers decide where unknown values land.
pub fn priority_value_rank(value: &str) -> Option<u8> {
    match value.to_lowercase().as_str() {
        "low" => Some(1),
        "medium" => Some(2),
        "high" => Some(3),
        "urgent" => Some(4),
        _ => None,
    }
}

impl Issue {
    /// The first label following the `priority:<value>` convention, if any.
    pub fn priority_label(&self) -> Option<&Label> {
        self.labels
            .iter()
            .find(|l| priority_value(&l.name).is_some())
    }

    /// Sort rank from the priority label: low = 1, medium = 2, high = 3,
    /// urgent = 4; no priority or an unknown value = 0.
    pub fn priority_rank(&self) -> u8 {
        self.priority_label()
            .and_then(|l| priority_value(&l.name))
            .and_then(priority_value_rank)
            .unwrap_or(0)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn issue_with_labels(labels: Vec<Label>) -> Issue {
        Issue {
            id: "id".into(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            url: String::new(),
            author: String::new(),
            assignees: vec![],
            labels,
            comment_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
        }
    }

    fn label(name: &str, color: &str) -> Label {
        Label {
            name: name.into(),
            color: color.into(),
        }
    }

    #[test]
    fn priority_label_found() {
        let issue = issue_with_labels(vec![
            label("bug", "d73a4a"),
            label("priority:high", "ff0000"),
        ]);
        assert_eq!(issue.priority_label().unwrap().name, "priority:high");
    }

    #[test]
    fn priority_label_absent() {
        let issue = issue_with_labels(vec![label("bug", "d73a4a")]);
        assert!(issue.priority_label().is_none());
    }

    #[test]
    fn priority_label_case_insensitive() {
        let issue = issue_with_labels(vec![label("Priority:High", "ff0000")]);
        assert_eq!(issue.priority_label().unwrap().name, "Priority:High");
    }

    #[test]
    fn priority_label_first_wins() {
        let issue = issue_with_labels(vec![
            label("priority:low", "00ff00"),
            label("priority:high", "ff0000"),
        ]);
        assert_eq!(issue.priority_label().unwrap().name, "priority:low");
    }

    #[test]
    fn bare_priority_label_does_not_match() {
        let issue = issue_with_labels(vec![label("priority", "ff0000")]);
        assert!(issue.priority_label().is_none());
    }

    #[test]
    fn priority_value_extracts_case_insensitively() {
        assert_eq!(priority_value("priority:high"), Some("high"));
        assert_eq!(priority_value("Priority:High"), Some("High"));
        assert_eq!(priority_value("bug"), None);
        assert_eq!(priority_value("priority"), None);
    }

    #[test]
    fn priority_value_rank_known_and_unknown() {
        assert_eq!(priority_value_rank("low"), Some(1));
        assert_eq!(priority_value_rank("Urgent"), Some(4));
        assert_eq!(priority_value_rank("P1"), None);
    }

    #[test]
    fn priority_rank_maps_known_values() {
        for (value, rank) in [("low", 1), ("medium", 2), ("high", 3), ("urgent", 4)] {
            let issue = issue_with_labels(vec![label(&format!("priority:{value}"), "")]);
            assert_eq!(issue.priority_rank(), rank, "value {value}");
        }
    }

    #[test]
    fn priority_rank_zero_without_priority() {
        assert_eq!(issue_with_labels(vec![]).priority_rank(), 0);
        assert_eq!(
            issue_with_labels(vec![label("bug", "d73a4a")]).priority_rank(),
            0
        );
    }

    #[test]
    fn priority_rank_zero_for_unknown_value() {
        assert_eq!(
            issue_with_labels(vec![label("priority:P1", "")]).priority_rank(),
            0
        );
    }

    #[test]
    fn priority_rank_is_case_insensitive() {
        assert_eq!(
            issue_with_labels(vec![label("Priority:High", "")]).priority_rank(),
            3
        );
    }
}
