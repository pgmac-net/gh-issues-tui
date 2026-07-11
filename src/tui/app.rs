use chrono::{DateTime, NaiveDate, Utc};

use crate::github::RateLimitData;
use crate::github::types::{Comment, Issue, IssueState, RepoIssues};

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

fn on_or_after(ts: Option<DateTime<Utc>>, bound: Option<NaiveDate>) -> bool {
    match bound {
        None => true,
        Some(b) => ts.is_some_and(|t| t.date_naive() >= b),
    }
}

fn on_or_before(ts: Option<DateTime<Utc>>, bound: Option<NaiveDate>) -> bool {
    match bound {
        None => true,
        Some(b) => ts.is_some_and(|t| t.date_naive() <= b),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub text: String,
    pub repo: String,
    pub assignee: String,
    pub author: String,
    /// Matches a `priority:<value>` label (bare value or full label name).
    pub priority: String,
    /// Matches a `status:<value>` label (bare value or full label name).
    pub status: String,
    pub created_after: Option<NaiveDate>,
    pub created_before: Option<NaiveDate>,
    pub updated_after: Option<NaiveDate>,
    pub updated_before: Option<NaiveDate>,
    pub closed_after: Option<NaiveDate>,
    pub closed_before: Option<NaiveDate>,
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

fn label_filter_matches(issue: &Issue, prefix: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let expected = format!("{prefix}:{filter}");
    issue
        .labels
        .iter()
        .any(|l| l.name.eq_ignore_ascii_case(filter) || l.name.eq_ignore_ascii_case(&expected))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Closed,
    State,
    Assignee,
    Author,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            SortKey::Updated => SortKey::Created,
            SortKey::Created => SortKey::Closed,
            SortKey::Closed => SortKey::State,
            SortKey::State => SortKey::Assignee,
            SortKey::Assignee => SortKey::Author,
            SortKey::Author => SortKey::Updated,
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
        };
        if descending { ord.reverse() } else { ord }
    });
}

/// A visible row in the main list: repo header or issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    RepoHeader { repo_idx: usize },
    Issue { repo_idx: usize, issue_idx: usize },
}

/// The filter-editor fields, in display order.
pub const FILTER_FIELDS: &[&str] = &[
    "text",
    "repo",
    "assignee",
    "author",
    "priority",
    "status",
    "created after (YYYY-MM-DD)",
    "created before",
    "updated after",
    "updated before",
    "closed after",
    "closed before",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Single-line text input; `kind` says what the submitted text does.
    Input(InputKind),
    /// Filter editor list.
    FilterMenu,
    /// Picking from a list of values for a filter field.
    SelectField(usize),
    /// Calendar date picker.
    Calendar(usize),
    /// y/n confirmation for close/reopen.
    ConfirmState,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    FilterField(usize),
    Comment,
    Assignees,
    Labels,
    Title,
    /// Switch the org/owner being browsed.
    Org,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

#[derive(Debug, Default)]
pub struct InputState {
    pub buffer: String,
    pub cursor: usize,
}

impl InputState {
    pub fn start(&mut self, initial: &str) {
        self.buffer = initial.to_string();
        self.cursor = self.buffer.chars().count();
    }

    pub fn insert(&mut self, c: char) {
        let byte = self.byte_at(self.cursor);
        self.buffer.insert(byte, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte = self.byte_at(self.cursor);
            self.buffer.remove(byte);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.buffer.chars().count());
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }
}

pub struct App {
    pub org: String,
    /// Raw data as fetched.
    pub repos: Vec<RepoIssues>,
    /// Collapsed repo names (survives reload).
    pub collapsed: std::collections::HashSet<String>,
    /// Repo names seen in any previous load; used to apply `default_collapsed`
    /// only to repos appearing for the first time.
    pub seen_repos: std::collections::HashSet<String>,
    /// Config: newly seen repos start collapsed.
    pub default_collapsed: bool,
    /// Visible rows derived from repos + filters + sort + collapsed.
    pub rows: Vec<Row>,
    pub selected: usize,
    pub state_filter: StateFilter,
    pub filters: Filters,
    pub sort_key: SortKey,
    pub sort_desc: bool,
    pub focus: Focus,
    pub mode: Mode,
    pub input: InputState,
    pub filter_menu_idx: usize,
    /// Available options for the current select-field popup.
    pub select_options: Vec<String>,
    /// Currently highlighted index in select_options.
    pub select_idx: usize,
    /// Cursor position for the calendar date picker.
    pub calendar_cursor: NaiveDate,
    pub loading: bool,
    pub include_closed: bool,
    pub status: Option<String>,
    /// Most recently observed API rate limit state.
    pub rate_limit: Option<RateLimitData>,
    /// Persistent rate-limit error (shown until cleared by a successful fetch).
    pub rate_limit_error: Option<String>,
    pub detail_comments: Option<Vec<Comment>>,
    pub detail_scroll: u16,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        org: String,
        initial_repo: Option<String>,
        include_closed: bool,
        default_collapsed: bool,
    ) -> Self {
        Self {
            org,
            repos: Vec::new(),
            collapsed: Default::default(),
            seen_repos: Default::default(),
            default_collapsed,
            rows: Vec::new(),
            selected: 0,
            state_filter: StateFilter::Open,
            filters: Filters {
                repo: initial_repo.unwrap_or_default(),
                ..Filters::default()
            },
            sort_key: SortKey::Updated,
            sort_desc: true,
            focus: Focus::List,
            mode: Mode::Normal,
            input: InputState::default(),
            filter_menu_idx: 0,
            select_options: Vec::new(),
            select_idx: 0,
            calendar_cursor: Utc::now().date_naive(),
            loading: true,
            include_closed,
            status: None,
            rate_limit: None,
            rate_limit_error: None,
            detail_comments: None,
            detail_scroll: 0,
            should_quit: false,
        }
    }

    pub fn set_data(&mut self, repos: Vec<RepoIssues>) {
        self.repos = repos;
        // First-seen repos take the configured default; repos the user has
        // already interacted with keep their manual collapse state. When the
        // current filters leave exactly one repo group visible, that group
        // defaults to expanded so its issues are immediately readable.
        let auto_expand = if self.default_collapsed {
            self.single_visible_repo()
        } else {
            None
        };
        for repo in &self.repos {
            if self.seen_repos.insert(repo.repo.clone())
                && self.default_collapsed
                && auto_expand.as_deref() != Some(repo.repo.as_str())
            {
                self.collapsed.insert(repo.repo.clone());
            }
        }
        self.loading = false;
        self.rebuild_rows();
    }

    /// True when the repo filter text exactly names a fetched repo — then
    /// `Filters::repo_matches` matches only that repo instead of substrings.
    fn repo_filter_exact(&self) -> bool {
        !self.filters.repo.is_empty()
            && self
                .repos
                .iter()
                .any(|r| r.repo.eq_ignore_ascii_case(&self.filters.repo))
    }

    /// Expand the lone visible repo group, if any. Called after every
    /// filter change so filtering down to one repo reveals its issues;
    /// a manual collapse afterwards sticks until the filters change again.
    pub fn expand_single_visible(&mut self) {
        if let Some(repo) = self.single_visible_repo()
            && self.collapsed.remove(&repo)
        {
            self.rebuild_rows();
        }
    }

    /// Name of the only repo group visible under the current filters, or
    /// `None` when zero or several groups are visible.
    fn single_visible_repo(&self) -> Option<String> {
        let exact = self.repo_filter_exact();
        let mut visible = self.repos.iter().filter(|r| {
            self.filters.repo_matches(&r.repo, exact)
                && r.issues
                    .iter()
                    .any(|i| self.filters.matches(i, self.state_filter))
        });
        let first = visible.next()?;
        visible.next().is_none().then(|| first.repo.clone())
    }

    /// Switch to browsing a different org/owner: drop all fetched data and
    /// per-org view state (filters, collapse, seen repos) for a fresh view.
    /// Keeps `include_closed` so the state-filter dataset stays consistent.
    pub fn switch_org(&mut self, org: String) {
        self.org = org;
        self.repos.clear();
        self.rows.clear();
        self.collapsed.clear();
        self.seen_repos.clear();
        self.filters.clear();
        self.state_filter = StateFilter::Open;
        self.selected = 0;
        self.focus = Focus::List;
        self.detail_comments = None;
        self.detail_scroll = 0;
        self.loading = true;
    }

    /// Recompute the visible rows. Keeps the selection in range.
    pub fn rebuild_rows(&mut self) {
        for repo in &mut self.repos {
            sort_issues(&mut repo.issues, self.sort_key, self.sort_desc);
        }
        self.rows.clear();
        let repo_exact = self.repo_filter_exact();
        for (ri, repo) in self.repos.iter().enumerate() {
            if !self.filters.repo_matches(&repo.repo, repo_exact) {
                continue;
            }
            let visible: Vec<usize> = repo
                .issues
                .iter()
                .enumerate()
                .filter(|(_, i)| self.filters.matches(i, self.state_filter))
                .map(|(idx, _)| idx)
                .collect();
            if visible.is_empty() {
                continue;
            }
            self.rows.push(Row::RepoHeader { repo_idx: ri });
            if !self.collapsed.contains(&repo.repo) {
                for ii in visible {
                    self.rows.push(Row::Issue {
                        repo_idx: ri,
                        issue_idx: ii,
                    });
                }
            }
        }
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        match self.rows.get(self.selected)? {
            Row::Issue {
                repo_idx,
                issue_idx,
            } => self.repos.get(*repo_idx)?.issues.get(*issue_idx),
            Row::RepoHeader { .. } => None,
        }
    }

    pub fn selected_repo(&self) -> Option<&RepoIssues> {
        match self.rows.get(self.selected)? {
            Row::Issue { repo_idx, .. } | Row::RepoHeader { repo_idx } => self.repos.get(*repo_idx),
        }
    }

    pub fn toggle_collapse(&mut self) {
        if let Some(repo) = self.selected_repo().map(|r| r.repo.clone()) {
            if !self.collapsed.remove(&repo) {
                self.collapsed.insert(repo);
            }
            self.rebuild_rows();
        }
    }

    pub fn set_current_collapsed(&mut self, collapsed: bool) {
        if let Some(repo) = self.selected_repo().map(|r| r.repo.clone()) {
            if collapsed {
                self.collapsed.insert(repo.clone());
            } else {
                self.collapsed.remove(&repo);
            }
            self.rebuild_rows();
            if collapsed {
                // Collapsing from a child row would leave the selection index
                // pointing at an unrelated row — land on the group's header.
                let header = self.rows.iter().position(|r| {
                    matches!(r, Row::RepoHeader { repo_idx }
                        if self.repos.get(*repo_idx).is_some_and(|ri| ri.repo == repo))
                });
                if let Some(idx) = header {
                    self.selected = idx;
                }
            }
        }
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        if collapsed {
            self.collapsed = self.repos.iter().map(|r| r.repo.clone()).collect();
        } else {
            self.collapsed.clear();
        }
        self.rebuild_rows();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Count of issues in a given repo that pass the current filters (excluding repo filter).
    pub fn repo_visible_count(&self, repo_idx: usize) -> usize {
        self.repos
            .get(repo_idx)
            .map(|repo| {
                repo.issues
                    .iter()
                    .filter(|i| self.filters.matches(i, self.state_filter))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Count of issues currently visible (excludes headers). Test helper —
    /// production code shows `filtered_issue_count` instead.
    #[cfg(test)]
    pub fn visible_issue_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, Row::Issue { .. }))
            .count()
    }

    /// Count of issues passing the current filters, including those hidden
    /// inside collapsed repo groups. Shown in the list title.
    pub fn filtered_issue_count(&self) -> usize {
        let exact = self.repo_filter_exact();
        self.repos
            .iter()
            .filter(|r| self.filters.repo_matches(&r.repo, exact))
            .flat_map(|r| r.issues.iter())
            .filter(|i| self.filters.matches(i, self.state_filter))
            .count()
    }

    /// Apply the submitted input buffer according to the active input kind.
    /// Returns the action the event loop must run, if any.
    pub fn apply_filter_input(&mut self, kind: InputKind, value: &str) {
        match kind {
            InputKind::Search => self.filters.text = value.to_string(),
            InputKind::FilterField(idx) => {
                let v = value.trim().to_string();
                match idx {
                    0 => self.filters.text = v,
                    1 => self.filters.repo = v,
                    2 => self.filters.assignee = v,
                    3 => self.filters.author = v,
                    4 => self.filters.priority = v,
                    5 => self.filters.status = v,
                    6 => self.filters.created_after = parse_date(&v),
                    7 => self.filters.created_before = parse_date(&v),
                    8 => self.filters.updated_after = parse_date(&v),
                    9 => self.filters.updated_before = parse_date(&v),
                    10 => self.filters.closed_after = parse_date(&v),
                    _ => self.filters.closed_before = parse_date(&v),
                }
            }
            _ => {}
        }
        self.rebuild_rows();
        self.expand_single_visible();
    }

    pub fn current_filter_value(&self, idx: usize) -> String {
        let d = |o: Option<NaiveDate>| o.map(|d| d.to_string()).unwrap_or_default();
        match idx {
            0 => self.filters.text.clone(),
            1 => self.filters.repo.clone(),
            2 => self.filters.assignee.clone(),
            3 => self.filters.author.clone(),
            4 => self.filters.priority.clone(),
            5 => self.filters.status.clone(),
            6 => d(self.filters.created_after),
            7 => d(self.filters.created_before),
            8 => d(self.filters.updated_after),
            9 => d(self.filters.updated_before),
            10 => d(self.filters.closed_after),
            _ => d(self.filters.closed_before),
        }
    }

    /// Build the list of options shown when the user presses Enter on a
    /// select-style filter field (repo, assignee, author, priority, status).
    /// The first entry is always `"—"` which means "no filter".
    pub fn compute_select_options(&self, idx: usize) -> Vec<String> {
        let mut opts: Vec<String> = match idx {
            1 => self.repos.iter().map(|r| r.repo.clone()).collect(),
            2 => {
                let mut v: Vec<String> = self
                    .repos
                    .iter()
                    .flat_map(|r| r.issues.iter())
                    .flat_map(|i| i.assignees.iter().cloned())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            3 => {
                let mut v: Vec<String> = self
                    .repos
                    .iter()
                    .flat_map(|r| r.issues.iter())
                    .map(|i| i.author.clone())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            4 => self.label_values("priority"),
            5 => self.label_values("status"),
            _ => vec![],
        };
        opts.insert(0, "\u{2014}".to_string());
        opts
    }

    /// Distinct sorted values of `<prefix>:<value>` labels across all issues.
    /// Splits on `:` rather than byte-slicing so mixed-case or non-ASCII
    /// label names can never panic on a char boundary.
    fn label_values(&self, prefix: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .flat_map(|i| i.labels.iter())
            .filter_map(|l| {
                l.name
                    .split_once(':')
                    .filter(|(p, _)| p.eq_ignore_ascii_case(prefix))
                    .map(|(_, value)| value.to_string())
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Returns `true` when the field at `idx` should show a selectable list
    /// instead of a free-text input.
    pub fn is_select_field(idx: usize) -> bool {
        matches!(idx, 1..=5)
    }

    /// Prepares the calendar cursor from the current filter value or today.
    pub fn calendar_init(&mut self, idx: usize) {
        let current = self.current_filter_value(idx);
        self.calendar_cursor = parse_date(&current).unwrap_or_else(|| Utc::now().date_naive());
    }

    /// Returns `true` when the field at `idx` uses the calendar date picker.
    pub fn is_calendar_field(idx: usize) -> bool {
        matches!(idx, 6..=11)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn issue(number: u64, title: &str, state: IssueState) -> Issue {
        Issue {
            id: format!("I_{number}"),
            number,
            title: title.into(),
            body: String::new(),
            state,
            url: format!("https://github.com/o/r/issues/{number}"),
            author: "pgmac".into(),
            assignees: vec![],
            labels: vec![],
            comment_count: 0,
            created_at: Utc
                .with_ymd_and_hms(2026, 6, number as u32 % 28 + 1, 0, 0, 0)
                .unwrap(),
            updated_at: Utc
                .with_ymd_and_hms(2026, 7, number as u32 % 28 + 1, 0, 0, 0)
                .unwrap(),
            closed_at: None,
        }
    }

    fn app_with(repos: Vec<RepoIssues>) -> App {
        let mut app = App::new("org".into(), None, false, false);
        app.set_data(repos);
        app
    }

    fn two_repo_app() -> App {
        app_with(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![
                    issue(1, "first bug", IssueState::Open),
                    issue(2, "feature idea", IssueState::Open),
                ],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(3, "docs fix", IssueState::Open)],
            },
        ])
    }

    #[test]
    fn rows_group_by_repo_with_headers() {
        let app = two_repo_app();
        assert_eq!(app.rows.len(), 5); // 2 headers + 3 issues
        assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));
        assert!(matches!(app.rows[3], Row::RepoHeader { repo_idx: 1 }));
    }

    #[test]
    fn collapse_hides_issue_rows_but_keeps_header() {
        let mut app = two_repo_app();
        app.selected = 0; // alpha header
        app.toggle_collapse();
        assert_eq!(app.rows.len(), 3); // alpha header + beta header + beta issue
        app.toggle_collapse();
        assert_eq!(app.rows.len(), 5);
    }

    #[test]
    fn default_collapsed_starts_all_groups_folded() {
        let mut app = App::new("org".into(), None, false, true);
        app.set_data(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(2, "b", IssueState::Open)],
            },
        ]);
        assert_eq!(app.rows.len(), 2); // headers only
        assert_eq!(app.visible_issue_count(), 0);
    }

    #[test]
    fn default_collapsed_preserves_manual_expand_across_reload() {
        let repos = || {
            vec![
                RepoIssues {
                    repo: "alpha".into(),
                    repo_url: "u".into(),
                    issues: vec![issue(1, "a", IssueState::Open)],
                },
                RepoIssues {
                    repo: "beta".into(),
                    repo_url: "u".into(),
                    issues: vec![issue(2, "b", IssueState::Open)],
                },
            ]
        };
        let mut app = App::new("org".into(), None, false, true);
        app.set_data(repos());
        assert_eq!(app.visible_issue_count(), 0);

        app.selected = 0;
        app.toggle_collapse(); // user expands alpha
        assert_eq!(app.visible_issue_count(), 1);

        app.set_data(repos()); // reload must not re-collapse it
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn default_collapsed_applies_to_new_repo_on_reload() {
        let alpha = RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        };
        let beta = RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(2, "b", IssueState::Open)],
        };
        let mut app = App::new("org".into(), None, false, true);
        app.set_data(vec![alpha.clone()]);
        assert!(!app.collapsed.contains("alpha")); // single group auto-expands

        app.set_data(vec![alpha, beta]); // beta appears for the first time
        assert!(!app.collapsed.contains("alpha"));
        assert!(app.collapsed.contains("beta"));
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn default_collapsed_single_repo_starts_expanded() {
        let mut app = App::new("org".into(), None, false, true);
        app.set_data(vec![RepoIssues {
            repo: "solo".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        }]);
        assert!(!app.collapsed.contains("solo"));
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn default_collapsed_expands_only_repo_matching_initial_filter() {
        let mut app = App::new("org".into(), Some("beta".into()), false, true);
        app.set_data(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(2, "b", IssueState::Open)],
            },
        ]);
        // beta is the single visible group → expanded; alpha still defaults
        // collapsed and shows once the filter is cleared.
        assert!(!app.collapsed.contains("beta"));
        assert!(app.collapsed.contains("alpha"));
        assert_eq!(app.visible_issue_count(), 1);

        app.filters.clear();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1); // beta open, alpha folded
        assert_eq!(app.rows.len(), 3); // two headers + beta's issue
    }

    #[test]
    fn manual_collapse_of_single_repo_survives_reload() {
        let repos = || {
            vec![RepoIssues {
                repo: "solo".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            }]
        };
        let mut app = App::new("org".into(), None, false, true);
        app.set_data(repos());
        assert_eq!(app.visible_issue_count(), 1); // auto-expanded

        app.selected = 0;
        app.toggle_collapse(); // user folds it
        assert_eq!(app.visible_issue_count(), 0);

        app.set_data(repos()); // reload must not force it back open
        assert_eq!(app.visible_issue_count(), 0);
    }

    #[test]
    fn without_default_collapsed_groups_start_expanded() {
        let app = two_repo_app(); // uses default_collapsed = false
        assert_eq!(app.visible_issue_count(), 3);
    }

    #[test]
    fn filtering_to_single_repo_expands_it() {
        let mut app = two_repo_app();
        app.set_all_collapsed(true);
        assert_eq!(app.visible_issue_count(), 0);

        // Repo filter leaving one visible group expands it.
        app.apply_filter_input(InputKind::FilterField(1), "beta");
        assert_eq!(app.visible_issue_count(), 1);
        assert!(!app.collapsed.contains("beta"));

        // Text search narrowing to one group expands too.
        app.set_all_collapsed(true);
        app.apply_filter_input(InputKind::FilterField(1), "");
        app.apply_filter_input(InputKind::Search, "docs");
        assert_eq!(app.visible_issue_count(), 1); // beta's "docs fix"
    }

    #[test]
    fn filtering_to_multiple_repos_keeps_them_folded() {
        let mut app = two_repo_app();
        app.set_all_collapsed(true);
        // "a" substring-matches both alpha and beta — no auto-expand.
        app.apply_filter_input(InputKind::FilterField(1), "a");
        assert_eq!(app.visible_issue_count(), 0);
        assert_eq!(app.rows.len(), 2); // two folded headers
    }

    #[test]
    fn manual_collapse_sticks_until_filters_change_again() {
        let mut app = two_repo_app();
        app.set_all_collapsed(true);
        app.apply_filter_input(InputKind::FilterField(1), "beta");
        assert_eq!(app.visible_issue_count(), 1); // auto-expanded

        app.selected = 0;
        app.toggle_collapse(); // user folds it — must stay folded
        assert_eq!(app.visible_issue_count(), 0);

        app.apply_filter_input(InputKind::Search, "docs"); // filters change
        assert_eq!(app.visible_issue_count(), 1); // re-expanded
    }

    #[test]
    fn filtered_issue_count_includes_collapsed_groups() {
        let mut app = two_repo_app(); // 3 issues across alpha + beta
        app.set_all_collapsed(true);
        assert_eq!(app.visible_issue_count(), 0);
        assert_eq!(app.filtered_issue_count(), 3);

        app.filters.repo = "beta".into();
        app.rebuild_rows();
        assert_eq!(app.filtered_issue_count(), 1);

        app.filters.clear();
        app.filters.text = "bug".into();
        app.rebuild_rows();
        assert_eq!(app.filtered_issue_count(), 1);
    }

    #[test]
    fn collapse_all_and_expand_all() {
        let mut app = two_repo_app();
        app.set_all_collapsed(true);
        assert_eq!(app.rows.len(), 2);
        app.set_all_collapsed(false);
        assert_eq!(app.rows.len(), 5);
    }

    #[test]
    fn text_filter_matches_title_and_number() {
        let mut app = two_repo_app();
        app.filters.text = "bug".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);

        app.filters.text = "#3".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);
        assert_eq!(app.rows.len(), 2); // beta header + issue 3
    }

    #[test]
    fn repo_filter_is_exact_when_text_names_a_repo() {
        let mut app = app_with(vec![
            RepoIssues {
                repo: "api".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            },
            RepoIssues {
                repo: "api-gateway".into(),
                repo_url: "u".into(),
                issues: vec![issue(2, "b", IssueState::Open)],
            },
        ]);
        app.filters.repo = "api".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);
        assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));

        // Case-insensitive exact match still wins over substring.
        app.filters.repo = "API".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);

        // No exact match → substring behavior matches both.
        app.filters.repo = "ap".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 2);
    }

    #[test]
    fn initial_repo_filter_applies_on_first_load() {
        let mut app = App::new("org".into(), Some("beta".into()), false, false);
        app.set_data(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(2, "b", IssueState::Open)],
            },
        ]);
        assert!(app.filters.is_active());
        assert_eq!(app.visible_issue_count(), 1);
        assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 1 }));

        app.filters.clear();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 2);
    }

    #[test]
    fn switch_org_resets_view_state() {
        let mut app = two_repo_app();
        app.filters.repo = "alpha".into();
        app.collapsed.insert("beta".into());
        app.state_filter = StateFilter::All;
        app.selected = 2;
        app.rebuild_rows();

        app.switch_org("other".into());
        assert_eq!(app.org, "other");
        assert!(app.repos.is_empty());
        assert!(app.rows.is_empty());
        assert!(app.collapsed.is_empty());
        assert!(app.seen_repos.is_empty());
        assert!(!app.filters.is_active());
        assert_eq!(app.state_filter, StateFilter::Open);
        assert_eq!(app.selected, 0);
        assert!(app.loading);
    }

    #[test]
    fn repo_filter_hides_whole_group() {
        let mut app = two_repo_app();
        app.filters.repo = "alph".into();
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 3);
        assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));
    }

    #[test]
    fn state_filter_cycles_and_filters() {
        let mut app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![
                issue(1, "open one", IssueState::Open),
                issue(2, "closed one", IssueState::Closed),
            ],
        }]);
        assert_eq!(app.visible_issue_count(), 1);
        app.state_filter = app.state_filter.next(); // closed
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);
        app.state_filter = app.state_filter.next(); // all
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 2);
    }

    #[test]
    fn assignee_and_author_filters() {
        let mut a = issue(1, "a", IssueState::Open);
        a.assignees = vec!["pgmac".into()];
        let mut b = issue(2, "b", IssueState::Open);
        b.author = "someone".into();
        let mut app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a, b],
        }]);

        app.filters.assignee = "PGMAC".into(); // case-insensitive
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);

        app.filters.clear();
        app.filters.author = "someone".into();
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn date_filters_bound_created() {
        let mut app = two_repo_app(); // created 2026-06-02, 06-03, 06-04
        app.filters.created_after = parse_date("2026-06-03");
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 2);
        app.filters.created_before = parse_date("2026-06-03");
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn closed_date_filter_excludes_never_closed() {
        let mut app = two_repo_app();
        app.filters.closed_after = parse_date("2020-01-01");
        app.rebuild_rows();
        assert_eq!(app.visible_issue_count(), 0);
    }

    #[test]
    fn sort_by_created_ascending_and_descending() {
        let mut issues = vec![
            issue(3, "c", IssueState::Open),
            issue(1, "a", IssueState::Open),
            issue(2, "b", IssueState::Open),
        ];
        sort_issues(&mut issues, SortKey::Created, false);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        sort_issues(&mut issues, SortKey::Created, true);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn sort_by_author() {
        let mut a = issue(1, "a", IssueState::Open);
        a.author = "zed".into();
        let mut b = issue(2, "b", IssueState::Open);
        b.author = "amy".into();
        let mut issues = vec![a, b];
        sort_issues(&mut issues, SortKey::Author, false);
        assert_eq!(issues[0].author, "amy");
    }

    #[test]
    fn selection_clamps_after_filter_shrinks_rows() {
        let mut app = two_repo_app();
        app.selected = 4;
        app.filters.text = "docs".into();
        app.rebuild_rows();
        assert!(app.selected < app.rows.len());
    }

    #[test]
    fn selected_issue_none_on_header() {
        let mut app = two_repo_app();
        app.selected = 0;
        assert!(app.selected_issue().is_none());
        app.selected = 1;
        assert_eq!(app.selected_issue().unwrap().number, 2); // sorted updated desc
    }

    #[test]
    fn filter_input_round_trip() {
        let mut app = two_repo_app();
        app.apply_filter_input(InputKind::FilterField(4), "2026-06-03");
        assert_eq!(app.current_filter_value(4), "2026-06-03");
        app.apply_filter_input(InputKind::FilterField(4), "");
        assert_eq!(app.current_filter_value(4), "");
    }

    #[test]
    fn input_state_edits_utf8_safely() {
        let mut input = InputState::default();
        input.start("héllo");
        input.left();
        input.backspace(); // remove second 'l'
        assert_eq!(input.buffer, "hélo");
        input.insert('x'); // cursor sits before the final 'o'
        assert_eq!(input.buffer, "hélxo");
    }

    #[test]
    fn label_filter_matches_bare_value() {
        let mut issue = issue(1, "a", IssueState::Open);
        issue.labels = vec![crate::github::types::Label {
            name: "priority:high".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(&issue, "priority", "high"));
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            "priority:high"
        ));
        assert!(!super::label_filter_matches(&issue, "priority", "low"));
        assert!(super::label_filter_matches(&issue, "priority", ""));
    }

    #[test]
    fn label_filter_matches_status() {
        let mut issue = issue(2, "b", IssueState::Open);
        issue.labels = vec![crate::github::types::Label {
            name: "status:needs-review".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(
            &issue,
            "status",
            "needs-review"
        ));
        assert!(super::label_filter_matches(
            &issue,
            "status",
            "status:needs-review"
        ));
        assert!(!super::label_filter_matches(&issue, "status", "blocked"));
    }

    #[test]
    fn label_filter_matches_is_case_insensitive() {
        let mut issue = issue(3, "c", IssueState::Open);
        issue.labels = vec![crate::github::types::Label {
            name: "Priority:High".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(&issue, "priority", "high"));
        assert!(super::label_filter_matches(&issue, "priority", "HIGH"));
    }

    #[test]
    fn compute_repo_options() {
        let app = two_repo_app();
        let opts = app.compute_select_options(1);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], "\u{2014}");
        assert!(opts.contains(&"alpha".to_string()));
        assert!(opts.contains(&"beta".to_string()));
    }

    #[test]
    fn compute_assignee_options() {
        let mut a = issue(1, "a", IssueState::Open);
        a.assignees = vec!["bob".into(), "alice".into()];
        let mut b = issue(2, "b", IssueState::Open);
        b.assignees = vec!["bob".into()];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a, b],
        }]);
        let opts = app.compute_select_options(2);
        assert_eq!(opts[0], "\u{2014}");
        assert!(opts.contains(&"alice".to_string()));
        assert!(opts.contains(&"bob".to_string()));
        assert_eq!(opts.len(), 3);
    }

    #[test]
    fn compute_author_options() {
        let mut a = issue(1, "a", IssueState::Open);
        a.author = "pgmac".into();
        let mut b = issue(2, "b", IssueState::Open);
        b.author = "someone".into();
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a, b],
        }]);
        let opts = app.compute_select_options(3);
        assert_eq!(opts[0], "\u{2014}");
        assert!(opts.contains(&"pgmac".to_string()));
        assert!(opts.contains(&"someone".to_string()));
        assert_eq!(opts.len(), 3);
    }

    #[test]
    fn compute_priority_options() {
        let mut a = issue(1, "a", IssueState::Open);
        a.labels = vec![crate::github::types::Label {
            name: "priority:high".into(),
            color: "".into(),
        }];
        let mut b = issue(2, "b", IssueState::Open);
        b.labels = vec![crate::github::types::Label {
            name: "priority:low".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a, b],
        }]);
        let opts = app.compute_select_options(4);
        assert_eq!(opts[0], "\u{2014}");
        assert!(opts.contains(&"high".to_string()));
        assert!(opts.contains(&"low".to_string()));
        assert_eq!(opts.len(), 3);
    }

    #[test]
    fn compute_status_options() {
        let mut a = issue(1, "a", IssueState::Open);
        a.labels = vec![crate::github::types::Label {
            name: "status:needs-review".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a],
        }]);
        let opts = app.compute_select_options(5);
        assert_eq!(opts[0], "\u{2014}");
        assert!(opts.contains(&"needs-review".to_string()));
        assert_eq!(opts.len(), 2);
    }

    #[test]
    fn compute_select_options_empty_when_no_label_match() {
        let app = two_repo_app();
        let opts = app.compute_select_options(4);
        assert_eq!(opts, vec!["\u{2014}".to_string()]);
    }

    #[test]
    fn collapse_from_child_row_selects_group_header() {
        let mut app = two_repo_app();
        app.selected = 2; // second issue inside alpha
        app.set_current_collapsed(true);
        assert_eq!(app.selected, 0); // alpha header
        assert!(matches!(
            app.rows[app.selected],
            Row::RepoHeader { repo_idx: 0 }
        ));
    }

    #[test]
    fn expand_via_set_current_collapsed_keeps_selection() {
        let mut app = two_repo_app();
        app.selected = 0;
        app.set_current_collapsed(true);
        app.set_current_collapsed(false);
        assert_eq!(app.selected, 0);
        assert_eq!(app.visible_issue_count(), 3);
    }

    #[test]
    fn label_values_handles_mixed_case_prefix() {
        let mut a = issue(1, "a", IssueState::Open);
        a.labels = vec![crate::github::types::Label {
            name: "Priority:High".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a],
        }]);
        let opts = app.compute_select_options(4);
        assert_eq!(opts, vec!["\u{2014}".to_string(), "High".to_string()]);
    }

    #[test]
    fn is_select_field_returns_correct_bool() {
        assert!(!App::is_select_field(0)); // text
        assert!(App::is_select_field(1)); // repo
        assert!(App::is_select_field(2)); // assignee
        assert!(App::is_select_field(3)); // author
        assert!(App::is_select_field(4)); // priority
        assert!(App::is_select_field(5)); // status
        assert!(!App::is_select_field(6)); // created after
    }

    #[test]
    fn parse_date_rejects_garbage() {
        assert!(parse_date("not-a-date").is_none());
        assert!(parse_date("").is_none());
        assert_eq!(
            parse_date("2026-07-05"),
            NaiveDate::from_ymd_opt(2026, 7, 5)
        );
    }
}
