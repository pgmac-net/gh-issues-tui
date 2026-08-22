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
    /// Points charged by the last completed fetch, when the backend reports a
    /// per-request cost (GitHub's `rateLimit.cost`). `None` for backends that
    /// don't — the field is filled in by `RateLimitStore::get`, never by the
    /// header parse that builds the rest of this struct.
    pub last_cost: Option<u64>,
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

/// A reference to a pull request *or* an issue: `github.com/{owner}/{repo}/pull/{N}`
/// links and the `{owner}/{repo}#{N}` and `#{N}` shorthands all parse to this.
/// `owner`/`repo` come from the reference itself, except for bare `#{N}`, which
/// inherits the repo of the thread it was written in.
///
/// The name is historical. A reference is only *known* to be a pull request
/// once [`crate::provider::IssueProvider::pull_request`] resolves it: GitHub
/// draws pull request and issue numbers from one per-repo sequence, so the
/// shorthand carries no way to tell the two apart. See [`PrLookup`].
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

/// The issue a [`PrRef`] turned out to name, when it was not a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
}

impl IssueRef {
    pub fn label(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// One resolved reference. Because a [`PrRef`] parsed from text is only a
/// candidate, fetching it can legitimately land on an issue instead — that is
/// not an error, and the popup offers to jump to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrLookup {
    Pr(Box<PrSummary>),
    Issue(IssueRef),
}

/// Characters allowed in a GitHub owner or repository name.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// Whether `prev` — the character before a shorthand — lets it start a
/// reference. Excluding `/` is what keeps the shorthand matcher off the tail of
/// a URL (`github.com/o/r#readme`), and excluding name characters is what keeps
/// `abc#1` and `v2.0#3` from matching.
fn is_ref_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | '[' | '{' | '<' | '"' | '\'' | ',' | ';' | ':' | '|' | '*'
                )
        }
    }
}

/// Whether the character after a shorthand's digits ends it. Rejecting a
/// trailing alphanumeric is what keeps `#12abc` and `#L12`-style anchors out.
fn ends_ref(next: Option<char>) -> bool {
    !matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == '_')
}

/// Leading run of name characters, and the rest of the input.
fn take_name(s: &str) -> Option<(String, &str)> {
    let end = s.find(|c: char| !is_name_char(c)).unwrap_or(s.len());
    (end > 0).then(|| (s[..end].to_string(), &s[end..]))
}

/// Leading run of digits as a number, and the rest of the input.
fn take_number(s: &str) -> Option<(u64, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse::<u64>().ok().map(|n| (n, &s[end..]))
}

/// `{owner}/{repo}/pull/{N}`, positioned just past the `github.com/` marker.
/// No terminator rule: a link may legitimately continue into a path or query
/// (`/pull/9/files?diff=split`).
fn match_pull_url(rest: &str) -> Option<(PrRef, usize)> {
    let (owner, r) = take_name(rest)?;
    let (repo, r) = take_name(r.strip_prefix('/')?)?;
    let (number, r) = take_number(r.strip_prefix("/pull/")?)?;
    Some((
        PrRef {
            owner,
            repo,
            number,
        },
        rest.len() - r.len(),
    ))
}

/// `{owner}/{repo}#{N}` shorthand.
fn match_qualified(rest: &str) -> Option<(PrRef, usize)> {
    let (owner, r) = take_name(rest)?;
    let (repo, r) = take_name(r.strip_prefix('/')?)?;
    let (number, r) = take_number(r.strip_prefix('#')?)?;
    ends_ref(r.chars().next()).then(|| {
        (
            PrRef {
                owner,
                repo,
                number,
            },
            rest.len() - r.len(),
        )
    })
}

/// `#{N}` shorthand, resolved against the repo whose thread is being read.
fn match_bare(rest: &str, current: Option<(&str, &str)>) -> Option<(PrRef, usize)> {
    let (owner, repo) = current?;
    let (number, r) = take_number(rest.strip_prefix('#')?)?;
    ends_ref(r.chars().next()).then(|| {
        (
            PrRef {
                owner: owner.to_string(),
                repo: repo.to_string(),
                number,
            },
            rest.len() - r.len(),
        )
    })
}

/// Backtick-delimited spans within one line, appended to `out` as absolute byte
/// ranges. A run of N backticks is closed by the next run of exactly N; an
/// unclosed run masks nothing. Backticks are ASCII, so byte indices here are
/// always char boundaries.
fn inline_code_ranges(line: &str, offset: usize, out: &mut Vec<std::ops::Range<usize>>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let open = i;
        while i < bytes.len() && bytes[i] == b'`' {
            i += 1;
        }
        let n = i - open;
        let mut j = i;
        let mut closed = None;
        while j < bytes.len() {
            if bytes[j] != b'`' {
                j += 1;
                continue;
            }
            let run = j;
            while j < bytes.len() && bytes[j] == b'`' {
                j += 1;
            }
            if j - run == n {
                closed = Some(j);
                break;
            }
        }
        match closed {
            Some(end) => {
                out.push(offset + open..offset + end);
                i = end;
            }
            None => break,
        }
    }
}

/// Byte ranges of fenced code blocks and inline code spans. These are excluded
/// from the scan: literal examples, diffs and hex colours (`#123456` is six
/// digits) live there and are not references. An unterminated fence masks the
/// rest of the text, which is the safe direction to err in.
fn code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut fence: Option<(usize, char, usize)> = None;
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next();
        let run = match marker {
            Some(c @ ('`' | '~')) => trimmed.chars().take_while(|&x| x == c).count(),
            _ => 0,
        };
        match fence {
            Some((start, fc, flen)) => {
                if marker == Some(fc) && run >= flen {
                    out.push(start..pos);
                    fence = None;
                }
            }
            None if run >= 3 => fence = Some((line_start, marker.expect("run >= 3"), run)),
            None => inline_code_ranges(line, line_start, &mut out),
        }
    }
    if let Some((start, _, _)) = fence {
        out.push(start..text.len());
    }
    out
}

/// Scan `text` for pull-request references, in first-seen order, deduped.
///
/// Three forms are recognised:
///
/// - `github.com/{owner}/{repo}/pull/{N}` — an explicit link.
/// - `{owner}/{repo}#{N}` — qualified shorthand.
/// - `#{N}` — bare shorthand, resolved against `current`, the `(owner, repo)`
///   of the thread being read. Ignored when `current` is `None`.
///
/// The two shorthands are ambiguous between issues and pull requests, since
/// GitHub numbers both from one per-repo sequence. A match is therefore a
/// *candidate*; the type is settled by fetching it (see [`PrLookup`]).
///
/// False positives are held down by three rules: matches inside fenced code
/// blocks and inline code spans are skipped, a shorthand must be preceded by a
/// boundary ([`is_ref_boundary`]), and its digits must be terminated
/// ([`ends_ref`]). One consequence worth knowing: `github.com/o/r#129` — a repo
/// URL with a numeric fragment — matches nothing, because the owner is
/// preceded by `/`. That same rule is what keeps the scanner out of URLs at
/// large, so the trade is deliberate.
pub fn parse_pr_links(text: &str, current: Option<(&str, &str)>) -> Vec<PrRef> {
    const MARKER: &str = "github.com/";
    let masks = code_ranges(text);

    let mut out: Vec<PrRef> = Vec::new();
    let mut prev: Option<char> = None;
    let mut i = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        let ch = rest.chars().next().expect("i is a char boundary below len");
        let matched = if masks.iter().any(|r| r.contains(&i)) {
            None
        } else if let Some(after) = rest.strip_prefix(MARKER) {
            match_pull_url(after).map(|(pr, len)| (pr, MARKER.len() + len))
        } else if is_ref_boundary(prev) {
            match_qualified(rest).or_else(|| match_bare(rest, current))
        } else {
            None
        };
        match matched {
            Some((pr, len)) => {
                if !out.contains(&pr) {
                    out.push(pr);
                }
                prev = text[..i + len].chars().next_back();
                i += len;
            }
            None => {
                prev = Some(ch);
                i += ch.len_utf8();
            }
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

    /// The repo a thread is being read in, for bare `#N` resolution.
    const HERE: Option<(&str, &str)> = Some(("o", "r"));

    #[test]
    fn parse_pr_links_full_url() {
        let text = "fixed by https://github.com/pgmac-net/gh-issues-tui/pull/72 thanks";
        assert_eq!(
            parse_pr_links(text, None),
            vec![pr("pgmac-net", "gh-issues-tui", 72)]
        );
    }

    #[test]
    fn parse_pr_links_multiple_preserves_order() {
        let text = "see https://github.com/o/r/pull/1 and https://github.com/o/r2/pull/2";
        assert_eq!(
            parse_pr_links(text, None),
            vec![pr("o", "r", 1), pr("o", "r2", 2)]
        );
    }

    #[test]
    fn parse_pr_links_dedupes() {
        let text = "https://github.com/o/r/pull/5 mentioned again: github.com/o/r/pull/5";
        assert_eq!(parse_pr_links(text, None), vec![pr("o", "r", 5)]);
    }

    #[test]
    fn parse_pr_links_trailing_path_and_query() {
        let text = "https://github.com/o/r/pull/9/files?diff=split and (github.com/o/r/pull/10)";
        assert_eq!(
            parse_pr_links(text, None),
            vec![pr("o", "r", 9), pr("o", "r", 10)]
        );
    }

    #[test]
    fn parse_pr_links_ignores_non_pull_github_urls() {
        let text = "https://github.com/o/r/issues/3 and https://github.com/o/r/commit/abc123";
        assert!(parse_pr_links(text, None).is_empty());
    }

    /// #129: the shortened form GitHub renders for cross-repo references.
    #[test]
    fn parse_pr_links_qualified_shorthand() {
        let text = "superseded by pgmac-net/gh-issues-tui#72 now";
        assert_eq!(
            parse_pr_links(text, HERE),
            vec![pr("pgmac-net", "gh-issues-tui", 72)]
        );
    }

    /// #129: bare `#N` means "this repo", so it can only resolve when the
    /// caller says which repo the thread belongs to.
    #[test]
    fn parse_pr_links_bare_shorthand_uses_the_current_repo() {
        let text = "closes #45, see also PR #72";
        assert_eq!(
            parse_pr_links(text, HERE),
            vec![pr("o", "r", 45), pr("o", "r", 72)]
        );
    }

    #[test]
    fn parse_pr_links_bare_shorthand_needs_a_current_repo() {
        assert!(parse_pr_links("closes #45", None).is_empty());
    }

    #[test]
    fn parse_pr_links_interleaves_forms_in_first_seen_order() {
        let text = "a/b#7 then https://github.com/o/r/pull/8 then #9";
        assert_eq!(
            parse_pr_links(text, HERE),
            vec![pr("a", "b", 7), pr("o", "r", 8), pr("o", "r", 9)]
        );
    }

    /// A thread routinely carries both a link and its shorthand for the same
    /// PR; they must collapse to one candidate.
    #[test]
    fn parse_pr_links_dedupes_a_url_against_its_own_shorthand() {
        let text = "https://github.com/o/r/pull/5 aka o/r#5 aka #5";
        assert_eq!(parse_pr_links(text, HERE), vec![pr("o", "r", 5)]);
    }

    #[test]
    fn parse_pr_links_requires_a_boundary_before_the_hash() {
        // `abc#1` is not a reference; `#L12` is a line anchor, not a number.
        assert!(parse_pr_links("abc#1 and #L12", HERE).is_empty());
    }

    #[test]
    fn parse_pr_links_requires_the_digits_to_be_terminated() {
        assert!(parse_pr_links("#12abc", HERE).is_empty());
    }

    /// The rule that keeps the scanner out of URLs also costs this case: a
    /// repo URL with a numeric fragment matches nothing, because the owner is
    /// preceded by `/`. Pinned so the trade-off is deliberate, not discovered.
    #[test]
    fn parse_pr_links_skips_a_repo_url_with_a_numeric_fragment() {
        assert!(parse_pr_links("https://github.com/o/r#129", HERE).is_empty());
    }

    #[test]
    fn parse_pr_links_skips_fenced_code_blocks() {
        let text = "before #1\n```\n#2\no/r#22\n```\nafter #3";
        assert_eq!(
            parse_pr_links(text, HERE),
            vec![pr("o", "r", 1), pr("o", "r", 3)]
        );
    }

    #[test]
    fn parse_pr_links_skips_inline_code_spans() {
        // `#123456` is a valid hex colour as well as a plausible issue number,
        // which is exactly why code spans are excluded.
        let text = "use `#123456` for the border, tracked in #5";
        assert_eq!(parse_pr_links(text, HERE), vec![pr("o", "r", 5)]);
    }

    #[test]
    fn parse_pr_links_treats_an_unterminated_fence_as_code_to_the_end() {
        let text = "real #1\n```\n#2\nstill code #3";
        assert_eq!(parse_pr_links(text, HERE), vec![pr("o", "r", 1)]);
    }
}
