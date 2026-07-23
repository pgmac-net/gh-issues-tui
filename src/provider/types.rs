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
    /// Backend node id, needed to edit the comment.
    pub id: String,
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

/// A reference to a pull request, parsed from a `github.com/{owner}/{repo}/pull/{N}`
/// link — owner/repo always come from the URL itself, never inferred.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    pub fn label(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }

    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.owner, self.repo, self.number
        )
    }
}

/// Scan `text` for explicit `github.com/{owner}/{repo}/pull/{N}` links.
/// Deliberately does not match bare `#N` shorthand — in an issues tool that's
/// ambiguous between an issue and a PR. Dedupes, preserving first-seen order.
pub fn parse_pr_links(text: &str) -> Vec<PrRef> {
    const MARKER: &str = "github.com/";
    let mut out: Vec<PrRef> = Vec::new();

    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(MARKER) {
        let start = search_from + rel + MARKER.len();
        search_from = start;
        let rest = &text[start..];
        let mut parts = rest.splitn(4, '/');
        let (Some(owner), Some(repo), Some("pull"), Some(after_pull)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let digits: String = after_pull
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let Ok(number) = digits.parse::<u64>() else {
            continue;
        };
        let pr = PrRef {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        };
        if !out.contains(&pr) {
            out.push(pr);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl std::fmt::Display for PrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrState::Open => write!(f, "open"),
            PrState::Closed => write!(f, "closed"),
            PrState::Merged => write!(f, "merged"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl std::fmt::Display for ReviewDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewDecision::Approved => write!(f, "approved"),
            ReviewDecision::ChangesRequested => write!(f, "changes requested"),
            ReviewDecision::ReviewRequired => write!(f, "review required"),
        }
    }
}

/// Latest review state per reviewer, plus GitHub's overall `reviewDecision`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewSummary {
    pub decision: Option<ReviewDecision>,
    pub approved: u32,
    pub changes_requested: u32,
    pub commented: u32,
}

/// One check run or legacy commit status, as shown under a PR's checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckContextInfo {
    pub name: String,
    /// Raw GitHub conclusion/state string (e.g. `SUCCESS`, `FAILURE`, `PENDING`).
    pub conclusion: String,
    /// Details/target URL for this check or status, opened by the PR
    /// summary popup's `o`/Enter action.
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckRollup {
    /// Overall rollup state, when GitHub reports one.
    pub state: Option<String>,
    pub contexts: Vec<CheckContextInfo>,
}

/// One Actions workflow run, either attached to the PR's head commit or to a
/// recent commit on the repo's default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunInfo {
    pub workflow: String,
    pub run_number: u64,
    pub event: String,
    pub conclusion: Option<String>,
    pub created_at: DateTime<Utc>,
    /// The run's URL on GitHub, opened by the PR summary popup's `o`/Enter
    /// action.
    pub url: String,
}

/// Everything the PR-summary popup needs, fetched in one GraphQL query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    pub pr: PrRef,
    pub title: String,
    pub body: String,
    pub state: PrState,
    pub is_draft: bool,
    pub base_ref: String,
    pub head_ref: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub comment_count: u64,
    pub review_thread_count: u64,
    pub reviews: ReviewSummary,
    pub checks: CheckRollup,
    pub pr_runs: Vec<WorkflowRunInfo>,
    pub default_branch_name: String,
    pub default_branch_runs: Vec<WorkflowRunInfo>,
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

    fn pr(owner: &str, repo: &str, number: u64) -> PrRef {
        PrRef {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }

    #[test]
    fn parse_pr_links_full_url() {
        let text = "fixed by https://github.com/pgmac-net/gh-issues-tui/pull/72 thanks";
        assert_eq!(
            parse_pr_links(text),
            vec![pr("pgmac-net", "gh-issues-tui", 72)]
        );
    }

    #[test]
    fn parse_pr_links_multiple_preserves_order() {
        let text = "see https://github.com/o/r/pull/1 and https://github.com/o/r2/pull/2";
        assert_eq!(
            parse_pr_links(text),
            vec![pr("o", "r", 1), pr("o", "r2", 2)]
        );
    }

    #[test]
    fn parse_pr_links_dedupes() {
        let text = "https://github.com/o/r/pull/5 mentioned again: github.com/o/r/pull/5";
        assert_eq!(parse_pr_links(text), vec![pr("o", "r", 5)]);
    }

    #[test]
    fn parse_pr_links_trailing_path_and_query() {
        let text = "https://github.com/o/r/pull/9/files?diff=split and (github.com/o/r/pull/10)";
        assert_eq!(
            parse_pr_links(text),
            vec![pr("o", "r", 9), pr("o", "r", 10)]
        );
    }

    #[test]
    fn parse_pr_links_ignores_non_pull_github_urls() {
        let text = "https://github.com/o/r/issues/3 and https://github.com/o/r/commit/abc123";
        assert!(parse_pr_links(text).is_empty());
    }

    #[test]
    fn parse_pr_links_ignores_bare_hash_shorthand() {
        let text = "closes #45, see also PR #72";
        assert!(parse_pr_links(text).is_empty());
    }
}
