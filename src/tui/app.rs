use chrono::{DateTime, NaiveDate, Utc};

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
        on_or_after(Some(issue.created_at), self.created_after)
            && on_or_before(Some(issue.created_at), self.created_before)
            && on_or_after(Some(issue.updated_at), self.updated_after)
            && on_or_before(Some(issue.updated_at), self.updated_before)
            && on_or_after(issue.closed_at, self.closed_after)
            && on_or_before(issue.closed_at, self.closed_before)
    }

    pub fn repo_matches(&self, repo: &str) -> bool {
        self.repo.is_empty() || repo.to_lowercase().contains(&self.repo.to_lowercase())
    }

    pub fn is_active(&self) -> bool {
        !self.text.is_empty()
            || !self.repo.is_empty()
            || !self.assignee.is_empty()
            || !self.author.is_empty()
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
    pub loading: bool,
    pub include_closed: bool,
    pub status: Option<String>,
    pub detail_comments: Option<Vec<Comment>>,
    pub detail_scroll: u16,
    pub should_quit: bool,
}

impl App {
    pub fn new(org: String, include_closed: bool) -> Self {
        Self {
            org,
            repos: Vec::new(),
            collapsed: Default::default(),
            rows: Vec::new(),
            selected: 0,
            state_filter: StateFilter::Open,
            filters: Filters::default(),
            sort_key: SortKey::Updated,
            sort_desc: true,
            focus: Focus::List,
            mode: Mode::Normal,
            input: InputState::default(),
            filter_menu_idx: 0,
            loading: true,
            include_closed,
            status: None,
            detail_comments: None,
            detail_scroll: 0,
            should_quit: false,
        }
    }

    pub fn set_data(&mut self, repos: Vec<RepoIssues>) {
        self.repos = repos;
        self.loading = false;
        self.rebuild_rows();
    }

    /// Recompute the visible rows. Keeps the selection in range.
    pub fn rebuild_rows(&mut self) {
        for repo in &mut self.repos {
            sort_issues(&mut repo.issues, self.sort_key, self.sort_desc);
        }
        self.rows.clear();
        for (ri, repo) in self.repos.iter().enumerate() {
            if !self.filters.repo_matches(&repo.repo) {
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

    /// Count of issues currently visible (excludes headers).
    pub fn visible_issue_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, Row::Issue { .. }))
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
                    4 => self.filters.created_after = parse_date(&v),
                    5 => self.filters.created_before = parse_date(&v),
                    6 => self.filters.updated_after = parse_date(&v),
                    7 => self.filters.updated_before = parse_date(&v),
                    8 => self.filters.closed_after = parse_date(&v),
                    _ => self.filters.closed_before = parse_date(&v),
                }
            }
            _ => {}
        }
        self.rebuild_rows();
    }

    pub fn current_filter_value(&self, idx: usize) -> String {
        let d = |o: Option<NaiveDate>| o.map(|d| d.to_string()).unwrap_or_default();
        match idx {
            0 => self.filters.text.clone(),
            1 => self.filters.repo.clone(),
            2 => self.filters.assignee.clone(),
            3 => self.filters.author.clone(),
            4 => d(self.filters.created_after),
            5 => d(self.filters.created_before),
            6 => d(self.filters.updated_after),
            7 => d(self.filters.updated_before),
            8 => d(self.filters.closed_after),
            _ => d(self.filters.closed_before),
        }
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
        let mut app = App::new("org".into(), false);
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
    fn parse_date_rejects_garbage() {
        assert!(parse_date("not-a-date").is_none());
        assert!(parse_date("").is_none());
        assert_eq!(
            parse_date("2026-07-05"),
            NaiveDate::from_ymd_opt(2026, 7, 5)
        );
    }
}
