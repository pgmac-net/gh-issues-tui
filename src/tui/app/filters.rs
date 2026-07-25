use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}

impl StateFilter {
    pub fn next(self) -> Self {
        match self {
            StateFilter::Open => StateFilter::Closed,
            StateFilter::Closed => StateFilter::All,
            StateFilter::All => StateFilter::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all",
        }
    }
}

/// One optional date bound. Parsed from `YYYY-MM-DD`.
pub fn parse_date(input: &str) -> Option<NaiveDate> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()
}

pub(super) fn on_or_after(ts: Option<DateTime<Utc>>, bound: Option<NaiveDate>) -> bool {
    match bound {
        None => true,
        Some(b) => ts.is_some_and(|t| t.date_naive() >= b),
    }
}

pub(super) fn on_or_before(ts: Option<DateTime<Utc>>, bound: Option<NaiveDate>) -> bool {
    match bound {
        None => true,
        Some(b) => ts.is_some_and(|t| t.date_naive() <= b),
    }
}

#[derive(Debug, Clone)]
pub struct Filters {
    pub text: String,
    pub repo: String,
    pub assignee: String,
    pub author: String,
    /// Matches any of these `priority:<value>` labels (bare values or full
    /// label names); empty means no filter.
    pub priority: Vec<String>,
    /// Matches any of these `status:<value>` labels (bare values or full
    /// label names); empty means no filter.
    pub status: Vec<String>,
    pub created_after: Option<NaiveDate>,
    pub created_before: Option<NaiveDate>,
    pub updated_after: Option<NaiveDate>,
    pub updated_before: Option<NaiveDate>,
    pub closed_after: Option<NaiveDate>,
    pub closed_before: Option<NaiveDate>,
    /// Hide repo groups with zero visible issues. Defaults true (today's
    /// clean view); `App::clear_filters`/`switch_org` restore the config
    /// default rather than this one.
    pub hide_empty: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            text: String::new(),
            repo: String::new(),
            assignee: String::new(),
            author: String::new(),
            priority: Vec::new(),
            status: Vec::new(),
            created_after: None,
            created_before: None,
            updated_after: None,
            updated_before: None,
            closed_after: None,
            closed_before: None,
            hide_empty: true,
        }
    }
}

impl Filters {
    pub fn matches(&self, issue: &Issue, state: StateFilter) -> bool {
        let state_ok = match state {
            StateFilter::All => true,
            StateFilter::Open => issue.state == IssueState::Open,
            StateFilter::Closed => issue.state == IssueState::Closed,
        };
        if !state_ok {
            return false;
        }
        if !self.text.is_empty() {
            let needle = self.text.to_lowercase();
            let hit = issue.title.to_lowercase().contains(&needle)
                || issue.body.to_lowercase().contains(&needle)
                || issue.number.to_string() == needle.trim_start_matches('#');
            if !hit {
                return false;
            }
        }
        if !self.assignee.is_empty()
            && !issue
                .assignees
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&self.assignee))
        {
            return false;
        }
        if !self.author.is_empty() && !issue.author.eq_ignore_ascii_case(&self.author) {
            return false;
        }
        if !label_filter_matches(issue, "priority", &self.priority) {
            return false;
        }
        if !label_filter_matches(issue, "status", &self.status) {
            return false;
        }
        on_or_after(Some(issue.created_at), self.created_after)
            && on_or_before(Some(issue.created_at), self.created_before)
            && on_or_after(Some(issue.updated_at), self.updated_after)
            && on_or_before(Some(issue.updated_at), self.updated_before)
            && on_or_after(issue.closed_at, self.closed_after)
            && on_or_before(issue.closed_at, self.closed_before)
    }

    /// `exact` is set when the filter text exactly names a fetched repo —
    /// then only that repo matches, so "api" can't drag in "api-gateway".
    /// Otherwise the filter is a case-insensitive substring.
    pub fn repo_matches(&self, repo: &str, exact: bool) -> bool {
        if self.repo.is_empty() {
            return true;
        }
        if exact {
            repo.eq_ignore_ascii_case(&self.repo)
        } else {
            repo.to_lowercase().contains(&self.repo.to_lowercase())
        }
    }

    pub fn is_active(&self) -> bool {
        !self.text.is_empty()
            || !self.repo.is_empty()
            || !self.assignee.is_empty()
            || !self.author.is_empty()
            || !self.priority.is_empty()
            || !self.status.is_empty()
            || self.created_after.is_some()
            || self.created_before.is_some()
            || self.updated_after.is_some()
            || self.updated_before.is_some()
            || self.closed_after.is_some()
            || self.closed_before.is_some()
    }

    pub fn clear(&mut self) {
        *self = Filters::default();
    }
}

/// Options for the set-priority picker: `—` (clear) first, then the repo's
/// `priority:*` labels ordered low → urgent with unknown values last,
/// alphabetical within a rank.
pub fn priority_set_options(repo_labels: &[RepoLabel]) -> Vec<String> {
    // Unknown priority values sort after the four known ranks.
    let rank = |name: &str| {
        priority_value(name)
            .and_then(priority_value_rank)
            .unwrap_or(5)
    };
    let mut prio: Vec<&str> = repo_labels
        .iter()
        .map(|l| l.name.as_str())
        .filter(|n| priority_value(n).is_some())
        .collect();
    prio.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
    let mut opts = vec!["\u{2014}".to_string()];
    opts.extend(prio.into_iter().map(String::from));
    opts
}

/// The issue's label names with any `priority:*` label replaced by `pick`,
/// or removed when `pick` is `None`.
pub fn priority_label_set(issue: &Issue, pick: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = issue
        .labels
        .iter()
        .map(|l| l.name.clone())
        .filter(|n| priority_value(n).is_none())
        .collect();
    if let Some(p) = pick {
        names.push(p.to_string());
    }
    names
}

/// Comma-separated text → filter values (trimmed, empties dropped). The
/// free-text path into the priority/status filters.
pub(super) fn parse_filter_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

pub(super) fn label_filter_matches(issue: &Issue, prefix: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|filter| {
        let expected = format!("{prefix}:{filter}");
        issue
            .labels
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(filter) || l.name.eq_ignore_ascii_case(&expected))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Closed,
    State,
    Assignee,
    Author,
    Priority,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            SortKey::Updated => SortKey::Created,
            SortKey::Created => SortKey::Closed,
            SortKey::Closed => SortKey::State,
            SortKey::State => SortKey::Assignee,
            SortKey::Assignee => SortKey::Author,
            SortKey::Author => SortKey::Priority,
            SortKey::Priority => SortKey::Updated,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Updated => "updated",
            SortKey::Created => "created",
            SortKey::Closed => "closed",
            SortKey::State => "state",
            SortKey::Assignee => "assignee",
            SortKey::Author => "author",
            SortKey::Priority => "priority",
        }
    }
}

pub fn sort_issues(issues: &mut [Issue], key: SortKey, descending: bool) {
    issues.sort_by(|a, b| {
        let ord = match key {
            SortKey::Updated => a.updated_at.cmp(&b.updated_at),
            SortKey::Created => a.created_at.cmp(&b.created_at),
            SortKey::Closed => a.closed_at.cmp(&b.closed_at),
            SortKey::State => format!("{}", a.state).cmp(&format!("{}", b.state)),
            SortKey::Assignee => a.assignees.join(",").cmp(&b.assignees.join(",")),
            SortKey::Author => a.author.cmp(&b.author),
            SortKey::Priority => a.priority_rank().cmp(&b.priority_rank()),
        };
        let ord = if descending { ord.reverse() } else { ord };
        // Priority ties fall back to most-recently-updated first, in both directions.
        if ord == std::cmp::Ordering::Equal && key == SortKey::Priority {
            b.updated_at.cmp(&a.updated_at)
        } else {
            ord
        }
    });
}
