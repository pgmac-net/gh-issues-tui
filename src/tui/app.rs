use chrono::{DateTime, NaiveDate, Utc};

use crate::provider::types::RateLimitData;
use crate::provider::types::{
    Comment, FormOptions, IdName, Issue, IssueState, NewIssueParams, PrRef, PrSummary, RepoIssues,
    RepoLabel, parse_pr_links, priority_value, priority_value_rank,
};

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
fn parse_filter_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn label_filter_matches(issue: &Issue, prefix: &str, filters: &[String]) -> bool {
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
    "hide empty repos",
];

/// Index of the "hide empty repos" toggle row in `FILTER_FIELDS` — it is
/// flipped in place on Enter rather than opening an input or picker.
pub const FILTER_HIDE_EMPTY_IDX: usize = FILTER_FIELDS.len() - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Single-line text input; `kind` says what the submitted text does.
    Input(InputKind),
    /// Filter editor list.
    FilterMenu,
    /// Picking from a list of values for a filter field.
    SelectField(usize),
    /// Multi-select picker (Space toggles) for a filter field.
    SelectFieldMulti(usize),
    /// Calendar date picker.
    Calendar(usize),
    /// Confirmation popup for close/reopen.
    ConfirmState,
    /// New-issue form field list.
    IssueForm,
    /// Single-select popup for a new-issue form field.
    IssueFormSelect(usize),
    /// Multi-select popup (Space toggles) for a new-issue form field.
    IssueFormMulti(usize),
    /// Multi-line editor for the new issue's body.
    IssueFormBody,
    /// Multi-line editor for adding a comment to the selected issue.
    CommentEditor,
    /// Single-select popup choosing a priority label for the selected issue.
    PrioritySet,
    /// Multi-select popup editing the full label set of the selected issue.
    LabelsSet,
    /// Picker choosing which linked PR to summarise, when more than one link
    /// was found.
    PrPicker,
    /// Popup showing a linked PR's summary.
    PrSummary,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    FilterField(usize),
    Assignees,
    Title,
    /// Switch the org/owner being browsed.
    Org,
    /// Title field of the new-issue form.
    FormTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

/// Which element of the inline comment section (`Mode::CommentEditor`) has
/// keys: the multi-line editor itself, or one of its two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentFocus {
    Editor,
    Save,
    Cancel,
}

/// What the inline editor (`Mode::CommentEditor`) writes on save. All three
/// share the same multi-line-editor + Save/Cancel widget; only the mutation
/// and the header text differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorTarget {
    /// Add a new comment to the selected issue (`c`).
    NewComment,
    /// Edit an existing comment by its backend id (`e` on a comment card).
    EditComment { comment_id: String },
    /// Edit the selected issue's description (`e` on the body card).
    EditBody,
}

/// Which button has keys in the `Mode::ConfirmState` popup. Reset to `No`
/// each time the popup opens — the safe default if Enter is pressed without
/// looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    Yes,
    No,
}

/// The new-issue form fields, in display order. The row after the last
/// field is the `[Create issue]` action (`ISSUE_FORM_CREATE_ROW`).
pub const ISSUE_FORM_FIELDS: &[&str] = &[
    "title",
    "description",
    "assignees",
    "labels",
    "type",
    "priority",
    "project",
    "milestone",
];

/// Index of the `[Create issue]` row in the form.
pub const ISSUE_FORM_CREATE_ROW: usize = ISSUE_FORM_FIELDS.len();

/// State of the new-issue form. Selections index into the corresponding
/// `FormOptions` list (not the "—"-prefixed popup display list).
pub struct IssueForm {
    /// Repo the issue will be created in, captured when the form opened.
    pub repo: String,
    pub title: String,
    pub body: BodyEditor,
    pub assignees: std::collections::HashSet<usize>,
    pub labels: std::collections::HashSet<usize>,
    pub issue_type: Option<usize>,
    pub priority: Option<usize>,
    pub project: Option<usize>,
    pub milestone: Option<usize>,
    /// `None` while the per-repo options fetch is still in flight.
    pub options: Option<FormOptions>,
    pub field_idx: usize,
}

impl IssueForm {
    pub fn new(repo: String) -> Self {
        Self {
            repo,
            title: String::new(),
            body: BodyEditor::default(),
            assignees: Default::default(),
            labels: Default::default(),
            issue_type: None,
            priority: None,
            project: None,
            milestone: None,
            options: None,
            field_idx: 0,
        }
    }

    /// True for fields edited with the multi-select popup.
    pub fn is_multi_field(idx: usize) -> bool {
        matches!(idx, 2 | 3)
    }

    /// True for fields edited with the single-select popup.
    pub fn is_select_field(idx: usize) -> bool {
        matches!(idx, 4..=7)
    }

    /// Labels acting as priorities under the `priority:<value>` convention.
    pub fn priority_options(&self) -> Vec<&IdName> {
        self.options
            .as_ref()
            .map(|o| {
                o.labels
                    .iter()
                    .filter(|l| l.name.to_lowercase().starts_with("priority:"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The option list backing a select/multi field, as display names.
    pub fn field_options(&self, idx: usize) -> Vec<String> {
        let Some(o) = &self.options else {
            return Vec::new();
        };
        let names = |v: &[IdName]| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        match idx {
            2 => names(&o.users),
            3 => names(&o.labels),
            4 => names(&o.issue_types),
            5 => self
                .priority_options()
                .iter()
                .map(|l| l.name.clone())
                .collect(),
            6 => names(&o.projects),
            7 => names(&o.milestones),
            _ => Vec::new(),
        }
    }

    /// Current selection(s) of a field as display text for the form row.
    pub fn field_display(&self, idx: usize) -> String {
        let opts = self.field_options(idx);
        let pick = |sel: Option<usize>| sel.and_then(|i| opts.get(i).cloned()).unwrap_or_default();
        match idx {
            0 => self.title.clone(),
            1 => self.body.summary(),
            2 | 3 => {
                let set = if idx == 2 {
                    &self.assignees
                } else {
                    &self.labels
                };
                let mut picked: Vec<usize> = set.iter().copied().collect();
                picked.sort_unstable();
                picked
                    .into_iter()
                    .filter_map(|i| opts.get(i).cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            4 => pick(self.issue_type),
            5 => pick(self.priority),
            6 => pick(self.project),
            7 => pick(self.milestone),
            _ => String::new(),
        }
    }

    /// Set a single-select field; `None` clears it.
    pub fn set_single(&mut self, idx: usize, choice: Option<usize>) {
        match idx {
            4 => self.issue_type = choice,
            5 => self.priority = choice,
            6 => self.project = choice,
            7 => self.milestone = choice,
            _ => {}
        }
    }

    pub fn get_single(&self, idx: usize) -> Option<usize> {
        match idx {
            4 => self.issue_type,
            5 => self.priority,
            6 => self.project,
            7 => self.milestone,
            _ => None,
        }
    }

    pub fn multi_set(&self, idx: usize) -> &std::collections::HashSet<usize> {
        if idx == 2 {
            &self.assignees
        } else {
            &self.labels
        }
    }

    pub fn multi_set_mut(&mut self, idx: usize) -> &mut std::collections::HashSet<usize> {
        if idx == 2 {
            &mut self.assignees
        } else {
            &mut self.labels
        }
    }

    /// Assemble the create parameters. `None` until the title is non-empty
    /// and the options fetch has landed (repo id comes from it).
    pub fn build_params(&self) -> Option<NewIssueParams> {
        let o = self.options.as_ref()?;
        let title = self.title.trim();
        if title.is_empty() {
            return None;
        }
        let ids = |set: &std::collections::HashSet<usize>, from: &[IdName]| {
            let mut picked: Vec<usize> = set.iter().copied().collect();
            picked.sort_unstable();
            picked
                .into_iter()
                .filter_map(|i| from.get(i).map(|x| x.id.clone()))
                .collect::<Vec<String>>()
        };
        let mut label_ids = ids(&self.labels, &o.labels);
        if let Some(p) = self.priority
            && let Some(label) = self.priority_options().get(p).map(|l| l.id.clone())
            && !label_ids.contains(&label)
        {
            label_ids.push(label);
        }
        Some(NewIssueParams {
            repo_id: o.repo_id.clone(),
            title: title.to_string(),
            body: self.body.text().trim_end().to_string(),
            assignee_ids: ids(&self.assignees, &o.users),
            label_ids,
            milestone_id: self
                .milestone
                .and_then(|i| o.milestones.get(i))
                .map(|m| m.id.clone()),
            issue_type_id: self
                .issue_type
                .and_then(|i| o.issue_types.get(i))
                .map(|t| t.id.clone()),
            project_id: self
                .project
                .and_then(|i| o.projects.get(i))
                .map(|p| p.id.clone()),
        })
    }
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

    /// Delete/Ctrl+D: remove the character under the cursor.
    pub fn delete_char(&mut self) {
        if self.cursor < self.buffer.chars().count() {
            let byte = self.byte_at(self.cursor);
            self.buffer.remove(byte);
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    /// Ctrl+←: to the start of the current or previous word
    /// (whitespace-delimited).
    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Ctrl+→: to the end of the current or next word.
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Ctrl+W: delete the word before the cursor (and the whitespace
    /// between it and the cursor).
    pub fn delete_word_back(&mut self) {
        let end = self.byte_at(self.cursor);
        self.word_left();
        let start = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
    }

    /// Ctrl+U: delete from the cursor back to the start of the line.
    pub fn kill_to_start(&mut self) {
        let byte = self.byte_at(self.cursor);
        self.buffer.replace_range(..byte, "");
        self.cursor = 0;
    }

    /// Ctrl+K: delete from the cursor to the end of the line.
    pub fn kill_to_end(&mut self) {
        let byte = self.byte_at(self.cursor);
        self.buffer.truncate(byte);
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }

    /// Split at the cursor: everything before stays, the tail is returned.
    fn split_at_cursor(&mut self) -> String {
        let byte = self.byte_at(self.cursor);
        self.buffer.split_off(byte)
    }
}

/// Minimal multi-line editor for the issue body: one UTF-8-safe
/// `InputState` per line. Always holds at least one line.
#[derive(Debug)]
pub struct BodyEditor {
    pub lines: Vec<InputState>,
    /// Index of the line the cursor is on.
    pub line: usize,
}

impl Default for BodyEditor {
    fn default() -> Self {
        Self {
            lines: vec![InputState::default()],
            line: 0,
        }
    }
}

/// Logical line count of one rendered comment card: a header rule, the body
/// (one line per source line), a bottom rule, and a trailing blank. Shared by
/// `App::detail_card_offset` and `ui::draw_detail` so the card-scroll offsets
/// match what is drawn.
pub fn comment_card_lines(c: &Comment) -> usize {
    3 + c.body.lines().count()
}

impl BodyEditor {
    /// Prefill an editor with existing text, one `InputState` per line, cursor
    /// at the end of the last line. An empty string yields the `Default`
    /// single blank line.
    pub fn from_text(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        let lines: Vec<InputState> = text
            .split('\n')
            .map(|l| InputState {
                cursor: l.chars().count(),
                buffer: l.to_string(),
            })
            .collect();
        let line = lines.len() - 1;
        Self { lines, line }
    }

    pub fn insert(&mut self, c: char) {
        self.lines[self.line].insert(c);
    }

    /// Enter: split the current line at the cursor.
    pub fn newline(&mut self) {
        let tail = self.lines[self.line].split_at_cursor();
        self.line += 1;
        self.lines.insert(
            self.line,
            InputState {
                buffer: tail,
                cursor: 0,
            },
        );
    }

    /// Backspace: within a line deletes a char; at column 0 merges the
    /// line into the previous one.
    pub fn backspace(&mut self) {
        if self.lines[self.line].cursor > 0 {
            self.lines[self.line].backspace();
        } else if self.line > 0 {
            let removed = self.lines.remove(self.line);
            self.line -= 1;
            let prev = &mut self.lines[self.line];
            prev.cursor = prev.buffer.chars().count();
            prev.buffer.push_str(&removed.buffer);
        }
    }

    /// Delete/Ctrl+D: within a line deletes the char under the cursor; at
    /// the end of a line merges the next line up (mirror of backspace).
    pub fn delete_char(&mut self) {
        let cur = &self.lines[self.line];
        if cur.cursor < cur.buffer.chars().count() {
            self.lines[self.line].delete_char();
        } else if self.line + 1 < self.lines.len() {
            let removed = self.lines.remove(self.line + 1);
            self.lines[self.line].buffer.push_str(&removed.buffer);
        }
    }

    pub fn left(&mut self) {
        self.lines[self.line].left();
    }

    pub fn right(&mut self) {
        self.lines[self.line].right();
    }

    pub fn word_left(&mut self) {
        self.lines[self.line].word_left();
    }

    pub fn word_right(&mut self) {
        self.lines[self.line].word_right();
    }

    pub fn delete_word_back(&mut self) {
        self.lines[self.line].delete_word_back();
    }

    pub fn home(&mut self) {
        self.lines[self.line].home();
    }

    pub fn end(&mut self) {
        self.lines[self.line].end();
    }

    pub fn kill_to_start(&mut self) {
        self.lines[self.line].kill_to_start();
    }

    pub fn kill_to_end(&mut self) {
        self.lines[self.line].kill_to_end();
    }

    /// Up one *visual* row of the `width`-wrapped layout, clamping the
    /// column; a no-op on the first row.
    pub fn up_visual(&mut self, width: usize) {
        self.move_visual(width, -1);
    }

    /// Down one visual row; a no-op on the last.
    pub fn down_visual(&mut self, width: usize) {
        self.move_visual(width, 1);
    }

    fn move_visual(&mut self, width: usize, delta: isize) {
        let rows = wrap_lines(&self.lines, width);
        let (row_idx, col) = cursor_row(&rows, self.line, self.lines[self.line].cursor);
        let Some(target) = row_idx
            .checked_add_signed(delta)
            .filter(|t| *t < rows.len())
        else {
            return;
        };
        let row = rows[target];
        // On a non-final row the position `end` already belongs to the next
        // visual row, so the rightmost landing spot is one before it.
        let line_final = rows.get(target + 1).is_none_or(|n| n.line != row.line);
        let max = if line_final {
            row.end
        } else {
            row.end.saturating_sub(1)
        };
        self.line = row.line;
        self.lines[self.line].cursor = (row.start + col).min(max);
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.buffer.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One-line summary for the form row.
    pub fn summary(&self) -> String {
        let text = self.text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let first = trimmed.lines().next().unwrap_or_default();
        let extra = trimmed.lines().count().saturating_sub(1);
        if extra > 0 {
            format!("{first} (+{extra} more lines)")
        } else {
            first.to_string()
        }
    }
}

/// The body-editor popup's outer width; the inner (text) width is this
/// clamped to the frame minus the two border columns. One source of truth
/// for the renderer and the key handler so wrap geometry always agrees.
pub const BODY_POPUP_WIDTH: u16 = 76;

pub fn body_popup_width(frame_width: u16) -> u16 {
    BODY_POPUP_WIDTH.min(frame_width).saturating_sub(2)
}

/// The single-line input popup's outer width; inner width mirrors
/// `body_popup_width`'s clamp-minus-borders pattern.
pub const INPUT_POPUP_WIDTH: u16 = 60;

pub fn input_popup_width(frame_width: u16) -> u16 {
    INPUT_POPUP_WIDTH.min(frame_width).saturating_sub(2)
}

/// The inline comment section's inner (text) width: it lives in the detail
/// pane, which is the right 60% of the frame (see `ui::draw`'s 40/60 split),
/// minus the section's own border columns. One source of truth for the
/// renderer and the key handler so wrap geometry always agrees.
pub fn comment_pane_width(frame_width: u16) -> u16 {
    let right = (frame_width as u32 * 60 / 100) as u16;
    right.saturating_sub(2)
}

/// The char index to start displaying from so a single-line input's cursor
/// always stays within a `width`-wide window. Stateless: recomputed from
/// `cursor` and `width` each frame, so the window only moves when the
/// cursor's position relative to the current window requires it.
pub fn input_scroll_skip(cursor: usize, width: usize) -> usize {
    let width = width.max(1);
    cursor.saturating_sub(width.saturating_sub(1))
}

/// One visual row of the word-wrapped body: a char range of a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    /// Index into `BodyEditor::lines`.
    pub line: usize,
    /// Char range within that line (`start..end`).
    pub start: usize,
    pub end: usize,
}

/// Word-wrap every logical line at `width` chars: break after the last
/// whitespace fitting in the window, hard-break words longer than `width`.
/// An empty line yields one empty row; `width` of 0 is treated as 1.
pub fn wrap_lines(lines: &[InputState], width: usize) -> Vec<VisualRow> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for (line_idx, l) in lines.iter().enumerate() {
        let chars: Vec<char> = l.buffer.chars().collect();
        let mut start = 0;
        loop {
            if chars.len() - start <= width {
                rows.push(VisualRow {
                    line: line_idx,
                    start,
                    end: chars.len(),
                });
                break;
            }
            let window_end = start + width;
            let brk = (start..window_end)
                .rev()
                .find(|&i| chars[i].is_whitespace())
                .map(|i| i + 1) // the space stays on this row
                .unwrap_or(window_end); // no space: hard break
            rows.push(VisualRow {
                line: line_idx,
                start,
                end: brk,
            });
            start = brk;
        }
    }
    rows
}

/// The visual position of a cursor: `(row index, column within the row)`.
/// A cursor sitting exactly on a wrap boundary belongs to the start of the
/// following row, except at the very end of a logical line.
pub fn cursor_row(rows: &[VisualRow], line: usize, cursor: usize) -> (usize, usize) {
    for (idx, row) in rows.iter().enumerate() {
        if row.line != line {
            continue;
        }
        let line_final = rows.get(idx + 1).is_none_or(|next| next.line != line);
        if cursor < row.end || (cursor == row.end && line_final) {
            return (idx, cursor - row.start);
        }
    }
    (0, 0) // unreachable with a clamped cursor; safe fallback
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
    /// Config: default for the hide-empty-repos filter; restored on
    /// filter clear and org switch.
    pub hide_empty_default: bool,
    /// Template for the short reference `y` copies to the clipboard.
    /// Supports `{owner}`, `{repo}`, `{number}` placeholders.
    pub copy_format: String,
    /// Visible rows derived from repos + filters + sort + collapsed.
    pub rows: Vec<Row>,
    pub selected: usize,
    pub state_filter: StateFilter,
    pub filters: Filters,
    pub sort_key: SortKey,
    pub sort_desc: bool,
    pub focus: Focus,
    /// Whether the detail pane (right split) is open. `focus` says which
    /// pane has keyboard focus while it is.
    pub detail_open: bool,
    pub mode: Mode,
    pub input: InputState,
    pub filter_menu_idx: usize,
    /// Available options for the current select-field popup.
    pub select_options: Vec<String>,
    /// Highlighted position within the *filtered* picker view.
    pub select_idx: usize,
    /// Type-ahead filter narrowing the picker view; reset on picker open.
    pub select_filter: String,
    /// Working set of toggled indices for the multi-select popup
    /// (committed to the form on Enter, discarded on Esc).
    pub multi_selected: std::collections::HashSet<usize>,
    /// The new-issue form, present while it is open.
    pub issue_form: Option<IssueForm>,
    /// Multi-line editor backing `Mode::CommentEditor`; reset each time the
    /// editor opens or closes.
    pub comment_editor: BodyEditor,
    /// Which element of the comment section has keys; reset to `Editor`
    /// each time the editor opens.
    pub comment_focus: CommentFocus,
    /// What the inline editor writes on save (add comment / edit comment /
    /// edit body); set each time the editor opens.
    pub editor_target: EditorTarget,
    /// Which button has keys in the `Mode::ConfirmState` popup; reset to
    /// `No` each time the popup opens.
    pub confirm_choice: ConfirmChoice,
    /// Issue id the set-priority picker was requested for; guards against
    /// stale option responses and selection drift while options load.
    pub priority_pick_issue: Option<String>,
    /// Issue id the edit-labels picker was requested for; guards against
    /// stale option responses and selection drift while options load.
    pub label_pick_issue: Option<String>,
    /// Cursor position for the calendar date picker.
    pub calendar_cursor: NaiveDate,
    pub loading: bool,
    /// The in-flight fetch was started by the auto-refresh ticker, not a
    /// keypress — picks the quieter status wording when it lands.
    pub auto_refreshing: bool,
    pub include_closed: bool,
    pub status: Option<String>,
    /// Most recently observed API rate limit state.
    pub rate_limit: Option<RateLimitData>,
    /// Persistent rate-limit error (shown until cleared by a successful fetch).
    pub rate_limit_error: Option<String>,
    pub detail_comments: Option<Vec<Comment>>,
    pub detail_scroll: u16,
    /// Highlighted card in the detail pane: 0 = the issue body, 1..=N = the
    /// (card − 1)th comment. Drives `j/k` navigation and which card `e` edits.
    pub detail_card: usize,
    /// Candidate PR links, populated when more than one is found (`Mode::PrPicker`).
    pub pr_links: Vec<PrRef>,
    /// The PR currently being fetched or shown; guards against a stale
    /// `PrSummary` response landing after the target moved on.
    pub pr_target: Option<PrRef>,
    /// `None` while the summary fetch for `pr_target` is in flight.
    pub pr_summary: Option<Result<PrSummary, String>>,
    pub pr_scroll: u16,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        org: String,
        initial_repo: Option<String>,
        include_closed: bool,
        default_collapsed: bool,
        copy_format: String,
    ) -> Self {
        Self {
            org,
            repos: Vec::new(),
            collapsed: Default::default(),
            seen_repos: Default::default(),
            default_collapsed,
            hide_empty_default: true,
            copy_format,
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
            detail_open: false,
            mode: Mode::Normal,
            input: InputState::default(),
            filter_menu_idx: 0,
            select_options: Vec::new(),
            select_idx: 0,
            select_filter: String::new(),
            multi_selected: Default::default(),
            issue_form: None,
            comment_editor: BodyEditor::default(),
            comment_focus: CommentFocus::Editor,
            editor_target: EditorTarget::NewComment,
            confirm_choice: ConfirmChoice::No,
            priority_pick_issue: None,
            label_pick_issue: None,
            calendar_cursor: Utc::now().date_naive(),
            loading: true,
            auto_refreshing: false,
            include_closed,
            status: None,
            rate_limit: None,
            rate_limit_error: None,
            detail_comments: None,
            detail_scroll: 0,
            detail_card: 0,
            pr_links: Vec::new(),
            pr_target: None,
            pr_summary: None,
            pr_scroll: 0,
            should_quit: false,
        }
    }

    /// Open an option picker: set its options and initial highlight, and
    /// reset the type-ahead filter.
    pub fn start_picker(&mut self, options: Vec<String>, idx: usize) {
        self.select_options = options;
        self.select_idx = idx;
        self.select_filter.clear();
    }

    /// The picker view under the type-ahead filter: `(original index,
    /// text)` pairs matching case-insensitively. An empty filter shows
    /// everything.
    pub fn filtered_select(&self) -> Vec<(usize, &str)> {
        let needle = self.select_filter.to_lowercase();
        self.select_options
            .iter()
            .enumerate()
            .filter(|(_, o)| needle.is_empty() || o.to_lowercase().contains(&needle))
            .map(|(i, o)| (i, o.as_str()))
            .collect()
    }

    /// Index into `select_options` of the highlighted picker row, `None`
    /// when the filter matches nothing.
    pub fn picker_selected_original(&self) -> Option<usize> {
        self.filtered_select().get(self.select_idx).map(|(i, _)| *i)
    }

    /// Append a type-ahead character; the highlight jumps to the first match.
    pub fn picker_filter_push(&mut self, c: char) {
        self.select_filter.push(c);
        self.select_idx = 0;
    }

    pub fn picker_filter_backspace(&mut self) {
        self.select_filter.pop();
        self.clamp_picker_idx();
    }

    pub fn picker_filter_clear(&mut self) {
        self.select_filter.clear();
        self.clamp_picker_idx();
    }

    fn clamp_picker_idx(&mut self) {
        let len = self.filtered_select().len();
        if self.select_idx >= len {
            self.select_idx = len.saturating_sub(1);
        }
    }

    /// Open the new-issue form targeting `repo`. Options arrive later via
    /// `set_form_options`; the caller spawns that fetch.
    pub fn open_issue_form(&mut self, repo: String) {
        self.issue_form = Some(IssueForm::new(repo));
        self.mode = Mode::IssueForm;
    }

    /// Discard the form and return to Normal mode.
    pub fn cancel_issue_form(&mut self) {
        self.issue_form = None;
        self.mode = Mode::Normal;
    }

    /// Deliver a per-repo options fetch. Dropped when the form has been
    /// closed or retargeted since the fetch was spawned (stale response).
    pub fn set_form_options(&mut self, repo: &str, options: FormOptions) {
        if let Some(form) = &mut self.issue_form
            && form.repo == repo
        {
            form.options = Some(options);
        }
    }

    /// Whether a background auto-refresh may fire now: no fetch already in
    /// flight, no rate-limit lockout, and nothing being composed or
    /// confirmed (only the passive Normal and Help modes qualify).
    pub fn should_auto_refresh(&self) -> bool {
        !self.loading
            && self.rate_limit_error.is_none()
            && matches!(self.mode, Mode::Normal | Mode::Help)
    }

    pub fn set_data(&mut self, repos: Vec<RepoIssues>) {
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
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
        // Keep the highlight on the same issue across a refresh — new data
        // can insert/remove rows, and the index-based selection would
        // otherwise silently land elsewhere. A vanished issue keeps the
        // index clamped by `rebuild_rows`.
        if let Some(id) = prev_selected
            && let Some(idx) = self.rows.iter().position(|row| match row {
                Row::Issue {
                    repo_idx,
                    issue_idx,
                } => self
                    .repos
                    .get(*repo_idx)
                    .and_then(|r| r.issues.get(*issue_idx))
                    .is_some_and(|i| i.id == id),
                Row::RepoHeader { .. } => false,
            })
        {
            self.selected = idx;
        }
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
        self.clear_filters();
        self.state_filter = StateFilter::Open;
        self.selected = 0;
        self.focus = Focus::List;
        self.detail_open = false;
        self.detail_comments = None;
        self.detail_scroll = 0;
        self.detail_card = 0;
        self.clear_pr_state();
        self.loading = true;
    }

    /// Open the detail pane on the selected issue and focus it.
    pub fn open_detail(&mut self) {
        self.detail_open = true;
        self.focus = Focus::Detail;
        self.detail_scroll = 0;
        self.detail_card = 0;
        self.detail_comments = None;
        self.clear_pr_state();
    }

    /// `→` on an issue row: move focus into the detail pane, opening the
    /// split first when it is closed. Returns the issue id when the pane
    /// was newly opened and its comments need fetching. No-op (`None`)
    /// on repo header rows — there `→` keeps meaning "expand the group".
    pub fn enter_detail(&mut self) -> Option<String> {
        let id = self.selected_issue().map(|i| i.id.clone())?;
        if self.detail_open {
            self.focus = Focus::Detail;
            None
        } else {
            self.open_detail();
            Some(id)
        }
    }

    /// `c`: start (or restart) the inline comment editor for the selected
    /// issue. Opens the detail pane first when it is closed (auto-follow —
    /// `c` works the same from the list view as it does inside the pane).
    /// Returns the issue id when the pane was newly opened and its
    /// comments need fetching, mirroring `enter_detail`.
    pub fn start_comment_editor(&mut self) -> Option<String> {
        self.selected_issue()?;
        self.comment_editor = BodyEditor::default();
        self.comment_focus = CommentFocus::Editor;
        self.editor_target = EditorTarget::NewComment;
        self.mode = Mode::CommentEditor;
        self.enter_detail()
    }

    /// Number of navigable cards in the detail pane: the issue body (card 0)
    /// plus one per loaded comment.
    pub fn detail_card_count(&self) -> usize {
        1 + self.detail_comments.as_ref().map_or(0, Vec::len)
    }

    /// Move the detail-pane card highlight by `delta`, clamped to the card
    /// range, and scroll so the newly selected card's top is in view.
    pub fn move_detail_card(&mut self, delta: isize) {
        let last = self.detail_card_count().saturating_sub(1);
        let next = (self.detail_card as isize + delta).clamp(0, last as isize) as usize;
        self.detail_card = next;
        self.detail_scroll = self.detail_card_offset(next);
    }

    /// The visual-line offset of card `card`'s top in the detail paragraph.
    /// Mirrors the line layout in `ui::draw_detail`; keep the two in sync.
    /// (Wrapping is not modelled — long lines make this an under-estimate, so
    /// the selected card's header still scrolls into view, just not pinned to
    /// the very top.)
    pub fn detail_card_offset(&self, card: usize) -> u16 {
        if card == 0 {
            return 0;
        }
        let Some(issue) = self.selected_issue() else {
            return 0;
        };
        // Fixed meta block (title, state, assignees, blank) + body + blank.
        let mut offset = 4 + issue.body.lines().count() + 1;
        if let Some(comments) = &self.detail_comments {
            for c in comments.iter().take(card - 1) {
                offset += comment_card_lines(c);
            }
        }
        u16::try_from(offset).unwrap_or(u16::MAX)
    }

    /// `e`: edit the highlighted detail card — the issue body (card 0) or a
    /// comment. Opens the inline editor prefilled with the current content.
    /// No-op unless the detail pane is open on an issue.
    pub fn start_edit_selected_card(&mut self) {
        if !self.detail_open {
            return;
        }
        if self.detail_card == 0 {
            let Some(issue) = self.selected_issue() else {
                return;
            };
            self.comment_editor = BodyEditor::from_text(&issue.body);
            self.editor_target = EditorTarget::EditBody;
        } else {
            let idx = self.detail_card - 1;
            let Some(c) = self
                .detail_comments
                .as_ref()
                .and_then(|cs| cs.get(idx))
                .cloned()
            else {
                return;
            };
            self.comment_editor = BodyEditor::from_text(&c.body);
            self.editor_target = EditorTarget::EditComment { comment_id: c.id };
        }
        self.comment_focus = CommentFocus::Editor;
        self.mode = Mode::CommentEditor;
    }

    /// Close the detail pane, returning focus to the list.
    pub fn close_detail(&mut self) {
        self.detail_open = false;
        self.focus = Focus::List;
        self.clear_pr_state();
    }

    fn clear_pr_state(&mut self) {
        self.pr_links.clear();
        self.pr_target = None;
        self.pr_summary = None;
        self.pr_scroll = 0;
    }

    /// PR links referenced by the selected issue's body and its loaded
    /// comment thread, body first then comments in display order.
    pub fn collect_pr_links(&self) -> Vec<PrRef> {
        let mut text = String::new();
        if let Some(issue) = self.selected_issue() {
            text.push_str(&issue.body);
            text.push('\n');
        }
        if let Some(comments) = &self.detail_comments {
            for c in comments {
                text.push_str(&c.body);
                text.push('\n');
            }
        }
        parse_pr_links(&text)
    }

    /// Open the summary popup for a single PR; the caller spawns the fetch.
    pub fn open_pr_summary(&mut self, pr: PrRef) {
        self.pr_target = Some(pr);
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.mode = Mode::PrSummary;
    }

    /// Open a picker over several candidate PR links.
    pub fn open_pr_picker(&mut self, links: Vec<PrRef>) {
        self.select_options = links.iter().map(PrRef::label).collect();
        self.select_idx = 0;
        self.select_filter.clear();
        self.pr_links = links;
        self.mode = Mode::PrPicker;
    }

    /// Deliver a PR summary fetch. Dropped if `pr` is no longer the target
    /// (the popup was closed or retargeted before the response landed).
    pub fn set_pr_summary(&mut self, pr: &PrRef, result: Result<PrSummary, String>) {
        if self.pr_target.as_ref() == Some(pr) {
            self.pr_summary = Some(result);
        }
    }

    /// Close the PR summary popup, back to the detail pane.
    pub fn close_pr_summary(&mut self) {
        self.pr_target = None;
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.mode = Mode::Normal;
    }

    /// Close the PR picker without selecting anything.
    pub fn close_pr_picker(&mut self) {
        self.pr_links.clear();
        self.mode = Mode::Normal;
    }

    /// Tab / Shift+Tab: move focus to the other pane. With two panes the
    /// direction doesn't matter; no-op when the split is closed.
    pub fn cycle_focus(&mut self) {
        if self.detail_open {
            self.focus = match self.focus {
                Focus::List => Focus::Detail,
                Focus::Detail => Focus::List,
            };
        }
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
            if visible.is_empty() && self.filters.hide_empty {
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

    /// Short reference for the selected issue, rendered from
    /// `copy_format` (`{owner}`, `{repo}`, `{number}`). `None` when no
    /// issue is selected (e.g. a repo header row).
    pub fn selected_short_ref(&self) -> Option<String> {
        let issue = self.selected_issue()?;
        let repo = self.selected_repo()?;
        Some(
            self.copy_format
                .replace("{owner}", &self.org)
                .replace("{repo}", &repo.repo)
                .replace("{number}", &issue.number.to_string()),
        )
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
                    4 => self.filters.priority = parse_filter_list(&v),
                    5 => self.filters.status = parse_filter_list(&v),
                    6 => self.filters.created_after = parse_date(&v),
                    7 => self.filters.created_before = parse_date(&v),
                    8 => self.filters.updated_after = parse_date(&v),
                    9 => self.filters.updated_before = parse_date(&v),
                    10 => self.filters.closed_after = parse_date(&v),
                    11 => self.filters.closed_before = parse_date(&v),
                    // "hide empty repos" toggles in place, never via input.
                    _ => {}
                }
            }
            _ => {}
        }
        self.rebuild_rows();
        self.expand_single_visible();
    }

    /// Commit a multi-select filter field (priority, status) and recompute
    /// the visible rows. An empty `values` clears the filter.
    pub fn apply_multi_filter(&mut self, idx: usize, values: Vec<String>) {
        match idx {
            4 => self.filters.priority = values,
            5 => self.filters.status = values,
            _ => return,
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
            4 => self.filters.priority.join(", "),
            5 => self.filters.status.join(", "),
            6 => d(self.filters.created_after),
            7 => d(self.filters.created_before),
            8 => d(self.filters.updated_after),
            9 => d(self.filters.updated_before),
            10 => d(self.filters.closed_after),
            11 => d(self.filters.closed_before),
            _ => if self.filters.hide_empty { "yes" } else { "no" }.to_string(),
        }
    }

    /// Flip the hide-empty-repos filter and recompute the rows.
    pub fn toggle_hide_empty(&mut self) {
        self.filters.hide_empty = !self.filters.hide_empty;
        self.rebuild_rows();
        self.expand_single_visible();
    }

    /// Clear the filter editor back to its defaults — the hide-empty
    /// toggle returns to the *config* default, not blanket false.
    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.filters.hide_empty = self.hide_empty_default;
    }

    /// Whether the filters-active indicator should show: any text/date
    /// filter set, or the hide-empty toggle moved off its config default.
    pub fn filters_active(&self) -> bool {
        self.filters.is_active() || self.filters.hide_empty != self.hide_empty_default
    }

    /// Set the config-derived default for the hide-empty filter, applying
    /// it to the live filter too (called once at startup).
    pub fn set_hide_empty_default(&mut self, hide: bool) {
        self.hide_empty_default = hide;
        self.filters.hide_empty = hide;
    }

    /// Build the list of options shown when the user presses Enter on a
    /// select-style filter field (repo, assignee, author).
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
            _ => vec![],
        };
        opts.insert(0, "\u{2014}".to_string());
        opts
    }

    /// Options for a multi-select filter field (priority, status). No "—"
    /// row — clearing is deselecting everything. Priority values are
    /// ordered low → urgent with unknown values last (like the set-priority
    /// picker); status values stay alphabetical.
    pub fn compute_multi_options(&self, idx: usize) -> Vec<String> {
        match idx {
            4 => {
                let rank = |v: &str| priority_value_rank(v).unwrap_or(5);
                let mut v = self.label_values("priority");
                v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
                v
            }
            5 => self.label_values("status"),
            _ => vec![],
        }
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

    /// Returns `true` when the field at `idx` should show a single-select
    /// list instead of a free-text input.
    pub fn is_select_field(idx: usize) -> bool {
        matches!(idx, 1..=3)
    }

    /// Returns `true` when the field at `idx` should show a multi-select
    /// list (priority, status — several values OR together).
    pub fn is_multi_select_field(idx: usize) -> bool {
        matches!(idx, 4 | 5)
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
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
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
        let mut app = App::new(
            "org".into(),
            None,
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
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
        let mut app = App::new(
            "org".into(),
            None,
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
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
        let mut app = App::new(
            "org".into(),
            None,
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
        app.set_data(vec![alpha.clone()]);
        assert!(!app.collapsed.contains("alpha")); // single group auto-expands

        app.set_data(vec![alpha, beta]); // beta appears for the first time
        assert!(!app.collapsed.contains("alpha"));
        assert!(app.collapsed.contains("beta"));
        assert_eq!(app.visible_issue_count(), 1);
    }

    #[test]
    fn default_collapsed_single_repo_starts_expanded() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
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
        let mut app = App::new(
            "org".into(),
            Some("beta".into()),
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
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
        let mut app = App::new(
            "org".into(),
            None,
            false,
            true,
            "{owner}/{repo}#{number}".into(),
        );
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
    fn detail_pane_open_close_and_focus_cycle() {
        let mut app = two_repo_app();
        assert!(!app.detail_open);
        app.cycle_focus(); // split closed → no-op
        assert_eq!(app.focus, Focus::List);

        app.open_detail();
        assert!(app.detail_open);
        assert_eq!(app.focus, Focus::Detail);

        app.cycle_focus();
        assert_eq!(app.focus, Focus::List);
        app.cycle_focus();
        assert_eq!(app.focus, Focus::Detail);

        app.close_detail();
        assert!(!app.detail_open);
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn switch_org_closes_detail_pane() {
        let mut app = two_repo_app();
        app.open_detail();
        app.switch_org("other".into());
        assert!(!app.detail_open);
        assert_eq!(app.focus, Focus::List);
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
        let mut app = App::new(
            "org".into(),
            Some("beta".into()),
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
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

    fn priority_issue(number: u64, priority: Option<&str>) -> Issue {
        let mut i = issue(number, "t", IssueState::Open);
        if let Some(p) = priority {
            i.labels = vec![crate::provider::types::Label {
                name: format!("priority:{p}"),
                color: String::new(),
            }];
        }
        i
    }

    #[test]
    fn sort_by_priority_descending_and_ascending() {
        let mut issues = vec![
            priority_issue(1, Some("low")),
            priority_issue(2, Some("urgent")),
            priority_issue(3, Some("medium")),
            priority_issue(4, Some("high")),
            priority_issue(5, None),
        ];
        sort_issues(&mut issues, SortKey::Priority, true);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![2, 4, 3, 1, 5]
        );
        sort_issues(&mut issues, SortKey::Priority, false);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![5, 1, 3, 4, 2]
        );
    }

    #[test]
    fn sort_by_priority_unknown_value_ranks_with_unsorted() {
        let mut issues = vec![
            priority_issue(1, Some("P1")),
            priority_issue(2, Some("low")),
        ];
        sort_issues(&mut issues, SortKey::Priority, true);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn priority_ties_break_by_updated_desc_in_both_directions() {
        // updated_at grows with the issue number in the test helper.
        let mut issues = vec![
            priority_issue(1, Some("high")),
            priority_issue(3, Some("high")),
            priority_issue(2, Some("high")),
        ];
        sort_issues(&mut issues, SortKey::Priority, true);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
        sort_issues(&mut issues, SortKey::Priority, false);
        assert_eq!(
            issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn sort_key_cycle_covers_all_keys_and_wraps() {
        let mut key = SortKey::Updated;
        let mut seen = vec![key];
        loop {
            key = key.next();
            if key == SortKey::Updated {
                break;
            }
            seen.push(key);
        }
        assert_eq!(seen.len(), 7);
        assert!(seen.contains(&SortKey::Priority));
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
    fn selected_short_ref_none_on_header() {
        let mut app = two_repo_app();
        app.selected = 0;
        assert!(app.selected_short_ref().is_none());
    }

    #[test]
    fn selected_short_ref_default_format() {
        let mut app = two_repo_app();
        app.selected = 1; // "alpha" issue #2
        assert_eq!(app.selected_short_ref().unwrap(), "org/alpha#2");
    }

    #[test]
    fn selected_short_ref_custom_format() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.copy_format = "{repo}#{number} ({owner})".into();
        assert_eq!(app.selected_short_ref().unwrap(), "alpha#2 (org)");
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
    fn filter_input_priority_parses_comma_list() {
        let mut app = two_repo_app();
        app.apply_filter_input(InputKind::FilterField(4), "high, urgent, ,");
        assert_eq!(app.filters.priority, vec!["high", "urgent"]);
        assert_eq!(app.current_filter_value(4), "high, urgent");
    }

    #[test]
    fn apply_multi_filter_sets_and_clears() {
        let mut app = two_repo_app();
        app.apply_multi_filter(5, vec!["blocked".into(), "in-progress".into()]);
        assert_eq!(app.filters.status, vec!["blocked", "in-progress"]);
        assert!(app.filters.is_active());
        app.apply_multi_filter(5, Vec::new());
        assert!(app.filters.status.is_empty());
        assert!(!app.filters.is_active());
    }

    #[test]
    fn input_scroll_skip_keeps_cursor_in_window() {
        // Cursor within the first window: no scroll.
        assert_eq!(input_scroll_skip(0, 10), 0);
        assert_eq!(input_scroll_skip(9, 10), 0);
        // Cursor past the window: skip advances to keep it on the last column.
        assert_eq!(input_scroll_skip(10, 10), 1);
        assert_eq!(input_scroll_skip(25, 10), 16);
        // Zero width is treated as one column wide.
        assert_eq!(input_scroll_skip(5, 0), 5);
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

    fn input(text: &str, cursor: usize) -> InputState {
        InputState {
            buffer: text.to_string(),
            cursor,
        }
    }

    #[test]
    fn word_motion_is_whitespace_delimited() {
        let mut i = input("foo-bar  baz héllo", 18);
        i.word_left();
        assert_eq!(i.cursor, 13); // start of "héllo"
        i.word_left();
        assert_eq!(i.cursor, 9); // start of "baz"
        i.word_left();
        assert_eq!(i.cursor, 0); // "foo-bar" is one word
        i.word_right();
        assert_eq!(i.cursor, 7); // end of "foo-bar"
        i.word_right();
        assert_eq!(i.cursor, 12); // end of "baz"
    }

    #[test]
    fn delete_word_back_removes_word_and_gap() {
        let mut i = input("one two  three", 14);
        i.delete_word_back();
        assert_eq!(i.buffer, "one two  ");
        assert_eq!(i.cursor, 9);
        i.delete_word_back();
        assert_eq!(i.buffer, "one ");
        i.delete_word_back();
        assert_eq!(i.buffer, "");
        i.delete_word_back(); // no-op at start
        assert_eq!(i.buffer, "");
    }

    #[test]
    fn kill_to_start_and_end() {
        let mut i = input("héllo world", 6);
        i.kill_to_end();
        assert_eq!(i.buffer, "héllo ");
        assert_eq!(i.cursor, 6);
        i.cursor = 3;
        i.kill_to_start();
        assert_eq!(i.buffer, "lo ");
        assert_eq!(i.cursor, 0);
    }

    #[test]
    fn delete_char_under_cursor() {
        let mut i = input("héllo", 1);
        i.delete_char();
        assert_eq!(i.buffer, "hllo");
        assert_eq!(i.cursor, 1);
        i.end();
        i.delete_char(); // no-op at end
        assert_eq!(i.buffer, "hllo");
    }

    #[test]
    fn home_and_end() {
        let mut i = input("abc", 1);
        i.home();
        assert_eq!(i.cursor, 0);
        i.end();
        assert_eq!(i.cursor, 3);
    }

    #[test]
    fn body_delete_char_merges_next_line_at_eol() {
        let mut b = BodyEditor::default();
        for c in "ab".chars() {
            b.insert(c);
        }
        b.newline();
        for c in "cd".chars() {
            b.insert(c);
        }
        b.line = 0;
        b.lines[0].end();
        b.delete_char();
        assert_eq!(b.text(), "abcd");
        assert_eq!(b.lines.len(), 1);
    }

    #[test]
    fn wrap_lines_breaks_at_word_boundary() {
        let lines = vec![input("aaa bbb ccc", 0)];
        let rows = wrap_lines(&lines, 5);
        // "aaa bbb ccc" at width 5 → "aaa " / "bbb " / "ccc"
        assert_eq!(
            rows,
            vec![
                VisualRow {
                    line: 0,
                    start: 0,
                    end: 4
                },
                VisualRow {
                    line: 0,
                    start: 4,
                    end: 8
                },
                VisualRow {
                    line: 0,
                    start: 8,
                    end: 11
                },
            ]
        );
    }

    #[test]
    fn wrap_lines_hard_breaks_long_words_and_keeps_empty_lines() {
        let lines = vec![input("abcdefghij", 0), input("", 0)];
        let rows = wrap_lines(&lines, 4);
        assert_eq!(rows.len(), 4); // 3 hard-broken rows + 1 empty row
        assert_eq!(
            rows[0],
            VisualRow {
                line: 0,
                start: 0,
                end: 4
            }
        );
        assert_eq!(
            rows[2],
            VisualRow {
                line: 0,
                start: 8,
                end: 10
            }
        );
        assert_eq!(
            rows[3],
            VisualRow {
                line: 1,
                start: 0,
                end: 0
            }
        );
    }

    #[test]
    fn wrap_lines_exact_width_does_not_split() {
        let lines = vec![input("abcd", 0)];
        assert_eq!(wrap_lines(&lines, 4).len(), 1);
    }

    #[test]
    fn cursor_row_maps_wrap_boundary_to_next_row() {
        let lines = vec![input("aaa bbb", 0)];
        let rows = wrap_lines(&lines, 5); // rows: "aaa " / "bbb"
        assert_eq!(rows.len(), 2);
        assert_eq!(cursor_row(&rows, 0, 2), (0, 2));
        // Cursor at the boundary char index 4 belongs to the second row.
        assert_eq!(cursor_row(&rows, 0, 4), (1, 0));
        // End of the line stays on its final row.
        assert_eq!(cursor_row(&rows, 0, 7), (1, 3));
    }

    #[test]
    fn visual_up_down_walk_wrapped_rows() {
        let mut b = BodyEditor::default();
        for c in "aaa bbb ccc".chars() {
            b.insert(c);
        }
        // width 5 → rows "aaa " / "bbb " / "ccc"; cursor at end (11) = row 2 col 3.
        b.up_visual(5);
        assert_eq!(b.lines[0].cursor, 7); // row 1 col 3 = char 4+3
        b.up_visual(5);
        assert_eq!(b.lines[0].cursor, 3); // row 0 col 3
        b.up_visual(5); // no-op on first row
        assert_eq!(b.lines[0].cursor, 3);
        b.down_visual(5);
        assert_eq!(b.lines[0].cursor, 7);
        b.down_visual(5);
        assert_eq!(b.lines[0].cursor, 11);
        b.down_visual(5); // no-op on last row
        assert_eq!(b.lines[0].cursor, 11);
    }

    #[test]
    fn visual_down_crosses_logical_lines() {
        let mut b = BodyEditor::default();
        for c in "short".chars() {
            b.insert(c);
        }
        b.newline();
        for c in "aaaa bbbb".chars() {
            b.insert(c);
        }
        b.line = 0;
        b.lines[0].cursor = 5;
        b.down_visual(6); // into line 1's first row "aaaa "
        assert_eq!((b.line, b.lines[1].cursor), (1, 4)); // clamped to end-1 of non-final row
        b.down_visual(6); // into "bbbb"
        assert_eq!((b.line, b.lines[1].cursor), (1, 9));
    }

    fn repo_label(name: &str) -> RepoLabel {
        RepoLabel {
            id: format!("L_{name}"),
            name: name.into(),
        }
    }

    #[test]
    fn priority_set_options_filters_sorts_and_prepends_clear() {
        let labels = vec![
            repo_label("bug"),
            repo_label("priority:urgent"),
            repo_label("priority:low"),
            repo_label("priority:aardvark"),
            repo_label("priority:high"),
            repo_label("status:blocked"),
        ];
        assert_eq!(
            priority_set_options(&labels),
            vec![
                "\u{2014}",
                "priority:low",
                "priority:high",
                "priority:urgent",
                "priority:aardvark",
            ]
        );
    }

    #[test]
    fn priority_set_options_empty_repo_is_clear_only() {
        assert_eq!(priority_set_options(&[repo_label("bug")]), vec!["\u{2014}"]);
    }

    #[test]
    fn priority_label_set_replaces_existing_priority() {
        let mut i = issue(1, "a", IssueState::Open);
        i.labels = vec![
            crate::provider::types::Label {
                name: "bug".into(),
                color: "".into(),
            },
            crate::provider::types::Label {
                name: "Priority:Low".into(),
                color: "".into(),
            },
        ];
        assert_eq!(
            priority_label_set(&i, Some("priority:high")),
            vec!["bug", "priority:high"]
        );
        // None clears the priority and keeps everything else.
        assert_eq!(priority_label_set(&i, None), vec!["bug"]);
    }

    #[test]
    fn priority_label_set_adds_when_none_present() {
        let mut i = issue(1, "a", IssueState::Open);
        i.labels = vec![crate::provider::types::Label {
            name: "bug".into(),
            color: "".into(),
        }];
        assert_eq!(
            priority_label_set(&i, Some("priority:urgent")),
            vec!["bug", "priority:urgent"]
        );
    }

    /// Filter values for `label_filter_matches` tests.
    fn fv(vals: &[&str]) -> Vec<String> {
        vals.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn label_filter_matches_bare_value() {
        let mut issue = issue(1, "a", IssueState::Open);
        issue.labels = vec![crate::provider::types::Label {
            name: "priority:high".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["high"])
        ));
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["priority:high"])
        ));
        assert!(!super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["low"])
        ));
        assert!(super::label_filter_matches(&issue, "priority", &[]));
    }

    #[test]
    fn label_filter_matches_any_of_several_values() {
        let mut issue = issue(4, "d", IssueState::Open);
        issue.labels = vec![crate::provider::types::Label {
            name: "priority:urgent".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["high", "urgent"])
        ));
        assert!(!super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["high", "medium"])
        ));
    }

    #[test]
    fn label_filter_matches_status() {
        let mut issue = issue(2, "b", IssueState::Open);
        issue.labels = vec![crate::provider::types::Label {
            name: "status:needs-review".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(
            &issue,
            "status",
            &fv(&["needs-review"])
        ));
        assert!(super::label_filter_matches(
            &issue,
            "status",
            &fv(&["status:needs-review"])
        ));
        assert!(!super::label_filter_matches(
            &issue,
            "status",
            &fv(&["blocked"])
        ));
    }

    #[test]
    fn label_filter_matches_is_case_insensitive() {
        let mut issue = issue(3, "c", IssueState::Open);
        issue.labels = vec![crate::provider::types::Label {
            name: "Priority:High".into(),
            color: "".into(),
        }];
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["high"])
        ));
        assert!(super::label_filter_matches(
            &issue,
            "priority",
            &fv(&["HIGH"])
        ));
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
        a.labels = vec![crate::provider::types::Label {
            name: "priority:high".into(),
            color: "".into(),
        }];
        let mut b = issue(2, "b", IssueState::Open);
        b.labels = vec![crate::provider::types::Label {
            name: "priority:low".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a, b],
        }]);
        // Multi-select options: no "—" row, rank-ordered low → urgent.
        let opts = app.compute_multi_options(4);
        assert_eq!(opts, vec!["low".to_string(), "high".to_string()]);
    }

    #[test]
    fn compute_priority_options_rank_order_unknown_last() {
        let mut a = issue(1, "a", IssueState::Open);
        a.labels = ["priority:urgent", "priority:medium", "priority:P1"]
            .iter()
            .map(|n| crate::provider::types::Label {
                name: n.to_string(),
                color: "".into(),
            })
            .collect();
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a],
        }]);
        assert_eq!(
            app.compute_multi_options(4),
            vec!["medium".to_string(), "urgent".to_string(), "P1".to_string()]
        );
    }

    #[test]
    fn compute_status_options() {
        let mut a = issue(1, "a", IssueState::Open);
        a.labels = vec![crate::provider::types::Label {
            name: "status:needs-review".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a],
        }]);
        let opts = app.compute_multi_options(5);
        assert_eq!(opts, vec!["needs-review".to_string()]);
    }

    #[test]
    fn compute_multi_options_empty_when_no_label_match() {
        let app = two_repo_app();
        assert!(app.compute_multi_options(4).is_empty());
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
        a.labels = vec![crate::provider::types::Label {
            name: "Priority:High".into(),
            color: "".into(),
        }];
        let app = app_with(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![a],
        }]);
        let opts = app.compute_multi_options(4);
        assert_eq!(opts, vec!["High".to_string()]);
    }

    #[test]
    fn is_select_field_returns_correct_bool() {
        assert!(!App::is_select_field(0)); // text
        assert!(App::is_select_field(1)); // repo
        assert!(App::is_select_field(2)); // assignee
        assert!(App::is_select_field(3)); // author
        assert!(!App::is_select_field(4)); // priority is multi now
        assert!(!App::is_select_field(5)); // status is multi now
        assert!(!App::is_select_field(6)); // created after
        assert!(!App::is_multi_select_field(3)); // author
        assert!(App::is_multi_select_field(4)); // priority
        assert!(App::is_multi_select_field(5)); // status
        assert!(!App::is_multi_select_field(6)); // created after
    }

    #[test]
    fn enter_detail_on_header_is_none_and_keeps_pane_closed() {
        let mut app = two_repo_app();
        app.selected = 0; // repo header
        assert_eq!(app.enter_detail(), None);
        assert!(!app.detail_open);
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn enter_detail_opens_closed_pane_and_requests_comments() {
        let mut app = two_repo_app();
        app.selected = 1; // first issue row
        let expected = app.selected_issue().unwrap().id.clone();
        assert_eq!(app.enter_detail(), Some(expected));
        assert!(app.detail_open);
        assert_eq!(app.focus, Focus::Detail);
    }

    #[test]
    fn enter_detail_on_open_pane_just_moves_focus() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.open_detail();
        app.focus = Focus::List; // as after ← backing out
        assert_eq!(app.enter_detail(), None); // no comment refetch
        assert!(app.detail_open);
        assert_eq!(app.focus, Focus::Detail);
    }

    #[test]
    fn start_comment_editor_on_header_is_none_and_keeps_pane_closed() {
        let mut app = two_repo_app();
        app.selected = 0; // repo header
        assert_eq!(app.start_comment_editor(), None);
        assert!(!app.detail_open);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn start_comment_editor_opens_closed_pane_and_requests_comments() {
        let mut app = two_repo_app();
        app.selected = 1; // first issue row
        let expected = app.selected_issue().unwrap().id.clone();
        assert_eq!(app.start_comment_editor(), Some(expected));
        assert!(app.detail_open);
        assert_eq!(app.focus, Focus::Detail);
        assert_eq!(app.mode, Mode::CommentEditor);
        assert_eq!(app.comment_focus, CommentFocus::Editor);
    }

    #[test]
    fn start_comment_editor_on_open_pane_keeps_comments_and_skips_refetch() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.open_detail();
        app.detail_comments = Some(vec![]);
        assert_eq!(app.start_comment_editor(), None); // no comment refetch
        assert!(app.detail_open);
        assert_eq!(app.mode, Mode::CommentEditor);
        assert_eq!(app.detail_comments.as_ref().map(Vec::len), Some(0));
    }

    #[test]
    fn start_comment_editor_resets_stale_editor_content_and_focus() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.comment_editor.insert('x');
        app.comment_focus = CommentFocus::Save;
        app.start_comment_editor();
        assert_eq!(app.comment_editor.text(), "");
        assert_eq!(app.comment_focus, CommentFocus::Editor);
    }

    fn comment(id: &str, body: &str) -> Comment {
        Comment {
            id: id.into(),
            author: "octocat".into(),
            created_at: Utc.with_ymd_and_hms(2026, 7, 22, 13, 6, 0).unwrap(),
            body: body.into(),
        }
    }

    /// Detail pane open on issue #1 (empty body) with two comments loaded.
    fn detail_app_with_comments() -> App {
        let mut app = two_repo_app();
        app.selected = 1; // issue #1
        app.open_detail();
        app.detail_comments = Some(vec![
            comment("c1", "first\nsecond"),
            comment("c2", "only one line"),
        ]);
        app
    }

    #[test]
    fn body_editor_from_text_splits_lines_and_ends_cursor() {
        let b = BodyEditor::from_text("hello\nworld");
        assert_eq!(b.text(), "hello\nworld");
        assert_eq!(b.line, 1);
        assert_eq!(b.lines[1].cursor, 5); // end of "world"
    }

    #[test]
    fn body_editor_from_empty_text_is_default() {
        let b = BodyEditor::from_text("");
        assert_eq!(b.lines.len(), 1);
        assert_eq!(b.text(), "");
    }

    #[test]
    fn comment_card_lines_counts_rules_body_and_blank() {
        assert_eq!(comment_card_lines(&comment("c", "one line")), 4); // 3 + 1
        assert_eq!(comment_card_lines(&comment("c", "a\nb\nc")), 6); // 3 + 3
    }

    #[test]
    fn detail_card_count_is_body_plus_comments() {
        let app = detail_app_with_comments();
        assert_eq!(app.detail_card_count(), 3); // body + 2 comments
    }

    #[test]
    fn move_detail_card_clamps_and_sets_scroll() {
        let mut app = detail_app_with_comments();
        assert_eq!(app.detail_card, 0);
        app.move_detail_card(-1); // clamped at body
        assert_eq!(app.detail_card, 0);
        assert_eq!(app.detail_scroll, 0);

        app.move_detail_card(1);
        assert_eq!(app.detail_card, 1);
        // Empty body: base = 4 meta + 0 body + 1 blank = 5.
        assert_eq!(app.detail_scroll, 5);

        app.move_detail_card(1);
        assert_eq!(app.detail_card, 2);
        // Second comment starts after the first card (3 + 2 body lines = 5).
        assert_eq!(app.detail_scroll, 10);

        app.move_detail_card(5); // clamped at last comment
        assert_eq!(app.detail_card, 2);
    }

    #[test]
    fn start_edit_body_card_prefills_and_targets_body() {
        let mut app = detail_app_with_comments();
        if let Some(&Row::Issue {
            repo_idx,
            issue_idx,
        }) = app.rows.get(app.selected)
        {
            app.repos[repo_idx].issues[issue_idx].body = "current description".into();
        }
        app.detail_card = 0;
        app.start_edit_selected_card();
        assert_eq!(app.mode, Mode::CommentEditor);
        assert_eq!(app.editor_target, EditorTarget::EditBody);
        assert_eq!(app.comment_editor.text(), "current description");
    }

    #[test]
    fn start_edit_comment_card_prefills_and_targets_comment_id() {
        let mut app = detail_app_with_comments();
        app.detail_card = 1; // first comment
        app.start_edit_selected_card();
        assert_eq!(app.mode, Mode::CommentEditor);
        assert_eq!(
            app.editor_target,
            EditorTarget::EditComment {
                comment_id: "c1".into()
            }
        );
        assert_eq!(app.comment_editor.text(), "first\nsecond");
    }

    #[test]
    fn start_edit_selected_card_noop_when_pane_closed() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.start_edit_selected_card();
        assert_eq!(app.mode, Mode::Normal);
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

    fn select_issue(app: &mut App, id: &str) {
        let idx = app
            .rows
            .iter()
            .position(|row| match row {
                Row::Issue {
                    repo_idx,
                    issue_idx,
                } => app.repos[*repo_idx].issues[*issue_idx].id == id,
                Row::RepoHeader { .. } => false,
            })
            .expect("issue row present");
        app.selected = idx;
    }

    #[test]
    fn set_data_keeps_selection_on_same_issue() {
        let mut app = two_repo_app();
        select_issue(&mut app, "I_1");

        // Refresh delivers a new issue that sorts above the selected one.
        app.set_data(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![
                    issue(1, "first bug", IssueState::Open),
                    issue(2, "feature idea", IssueState::Open),
                    issue(5, "brand new", IssueState::Open),
                ],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(3, "docs fix", IssueState::Open)],
            },
        ]);

        assert_eq!(app.selected_issue().map(|i| i.id.as_str()), Some("I_1"));
    }

    #[test]
    fn set_data_clamps_when_selected_issue_vanishes() {
        let mut app = two_repo_app();
        select_issue(&mut app, "I_3"); // last row (beta's only issue)

        app.set_data(vec![RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "first bug", IssueState::Open)],
        }]);

        assert!(app.selected < app.rows.len());
        assert!(app.selected_issue().is_none_or(|i| i.id != "I_3"));
    }

    fn form_options() -> FormOptions {
        let id_name = |id: &str, name: &str| IdName {
            id: id.into(),
            name: name.into(),
        };
        FormOptions {
            repo_id: "R_repo".into(),
            labels: vec![
                id_name("L_bug", "bug"),
                id_name("L_enh", "enhancement"),
                id_name("L_ph", "priority:high"),
                id_name("L_pl", "priority:low"),
            ],
            users: vec![id_name("U_pgmac", "pgmac"), id_name("U_bot", "bot")],
            milestones: vec![id_name("M_1", "v1.0")],
            projects: vec![id_name("P_1", "Homelab")],
            issue_types: vec![id_name("T_bug", "Bug"), id_name("T_feat", "Feature")],
        }
    }

    #[test]
    fn issue_form_opens_and_options_land() {
        let mut app = two_repo_app();
        app.open_issue_form("alpha".into());
        assert_eq!(app.mode, Mode::IssueForm);
        let form = app.issue_form.as_ref().unwrap();
        assert_eq!(form.repo, "alpha");
        assert!(form.options.is_none());
        assert!(form.field_options(3).is_empty()); // loading → empty

        app.set_form_options("alpha", form_options());
        let form = app.issue_form.as_ref().unwrap();
        assert_eq!(form.field_options(2), vec!["pgmac", "bot"]);
        assert_eq!(form.field_options(5), vec!["priority:high", "priority:low"]);
    }

    #[test]
    fn stale_form_options_are_dropped() {
        let mut app = two_repo_app();
        app.open_issue_form("alpha".into());
        app.set_form_options("beta", form_options()); // stale: other repo
        assert!(app.issue_form.as_ref().unwrap().options.is_none());

        app.cancel_issue_form();
        app.set_form_options("alpha", form_options()); // stale: form closed
        assert!(app.issue_form.is_none());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn build_params_requires_options_and_title() {
        let mut form = IssueForm::new("alpha".into());
        form.title = "hello".into();
        assert!(form.build_params().is_none()); // options not loaded

        form.options = Some(form_options());
        form.title = "   ".into();
        assert!(form.build_params().is_none()); // blank title

        form.title = "hello".into();
        let p = form.build_params().unwrap();
        assert_eq!(p.repo_id, "R_repo");
        assert_eq!(p.title, "hello");
        assert!(p.label_ids.is_empty() && p.assignee_ids.is_empty());
        assert!(p.milestone_id.is_none() && p.issue_type_id.is_none() && p.project_id.is_none());
    }

    #[test]
    fn build_params_assembles_ids_and_merges_priority() {
        let mut form = IssueForm::new("alpha".into());
        form.options = Some(form_options());
        form.title = "t".into();
        form.assignees.insert(0); // pgmac
        form.labels.insert(0); // bug
        form.priority = Some(0); // priority:high → L_ph
        form.issue_type = Some(1); // Feature
        form.project = Some(0);
        form.milestone = Some(0);

        let p = form.build_params().unwrap();
        assert_eq!(p.assignee_ids, vec!["U_pgmac"]);
        assert_eq!(p.label_ids, vec!["L_bug", "L_ph"]);
        assert_eq!(p.issue_type_id.as_deref(), Some("T_feat"));
        assert_eq!(p.project_id.as_deref(), Some("P_1"));
        assert_eq!(p.milestone_id.as_deref(), Some("M_1"));

        // Picking the same priority label in the labels field must not
        // duplicate its id.
        form.labels.insert(2); // priority:high via labels
        let p = form.build_params().unwrap();
        assert_eq!(
            p.label_ids.iter().filter(|i| *i == "L_ph").count(),
            1,
            "priority label id duplicated"
        );
    }

    #[test]
    fn form_field_display_joins_multi_selections() {
        let mut form = IssueForm::new("alpha".into());
        form.options = Some(form_options());
        form.labels.insert(1);
        form.labels.insert(0);
        assert_eq!(form.field_display(3), "bug, enhancement");
        form.priority = Some(1);
        assert_eq!(form.field_display(5), "priority:low");
    }

    #[test]
    fn body_editor_splits_merges_and_clamps() {
        let mut b = BodyEditor::default();
        for c in "hello".chars() {
            b.insert(c);
        }
        b.left();
        b.left(); // cursor after "hel"
        b.newline();
        assert_eq!(b.text(), "hel\nlo");
        assert_eq!(b.line, 1);
        assert_eq!(b.lines[1].cursor, 0);

        b.backspace(); // col 0 → merge back
        assert_eq!(b.text(), "hello");
        assert_eq!(b.line, 0);
        assert_eq!(b.lines[0].cursor, 3); // at the old split point

        b.newline();
        b.insert('x');
        b.up_visual(80); // wide enough that visual rows == logical lines
        assert_eq!(b.line, 0);
        b.down_visual(80);
        assert_eq!(b.line, 1);
        assert_eq!(b.text(), "hel\nxlo");
        assert_eq!(b.summary(), "hel (+1 more lines)");
    }

    #[test]
    fn body_editor_handles_multibyte() {
        let mut b = BodyEditor::default();
        for c in "héllo".chars() {
            b.insert(c);
        }
        b.left();
        b.left();
        b.left(); // after "hé"
        b.newline();
        assert_eq!(b.text(), "hé\nllo");
        b.backspace();
        assert_eq!(b.text(), "héllo");
    }

    fn picker_app(options: &[&str]) -> App {
        let mut app = two_repo_app();
        app.start_picker(options.iter().map(|s| s.to_string()).collect(), 0);
        app
    }

    #[test]
    fn picker_filter_narrows_and_maps_to_original_indices() {
        let mut app = picker_app(&["\u{2014}", "ansible", "budgeteer", "gh-issues-tui", "ghar"]);
        app.picker_filter_push('g');
        app.picker_filter_push('h');
        let filtered = app.filtered_select();
        assert_eq!(
            filtered,
            vec![(3, "gh-issues-tui"), (4, "ghar")],
            "case-insensitive substring over original indices"
        );
        assert_eq!(app.select_idx, 0); // reset to first match
        assert_eq!(app.picker_selected_original(), Some(3));

        app.select_idx = 1;
        assert_eq!(app.picker_selected_original(), Some(4));
    }

    #[test]
    fn picker_filter_matches_case_insensitively() {
        let mut app = picker_app(&["Docker-Nagios", "homelabia"]);
        app.picker_filter_push('N');
        app.picker_filter_push('A');
        assert_eq!(app.filtered_select(), vec![(0, "Docker-Nagios")]);
    }

    #[test]
    fn picker_backspace_and_clear_restore_and_clamp() {
        let mut app = picker_app(&["alpha", "beta"]);
        app.select_idx = 1; // beta
        app.picker_filter_push('x'); // no matches
        assert!(app.filtered_select().is_empty());
        assert_eq!(app.picker_selected_original(), None);

        app.picker_filter_backspace();
        assert_eq!(app.filtered_select().len(), 2);
        assert!(app.select_idx < 2); // clamped into range

        app.picker_filter_push('b');
        app.picker_filter_clear();
        assert_eq!(app.select_filter, "");
        assert_eq!(app.filtered_select().len(), 2);
    }

    #[test]
    fn start_picker_resets_filter() {
        let mut app = picker_app(&["alpha"]);
        app.picker_filter_push('z');
        app.start_picker(vec!["beta".into()], 0);
        assert_eq!(app.select_filter, "");
        assert_eq!(app.filtered_select(), vec![(0, "beta")]);
    }

    fn app_with_empty_repo() -> App {
        app_with(vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "first bug", IssueState::Open)],
            },
            RepoIssues {
                repo: "empty-repo".into(),
                repo_url: "u".into(),
                issues: vec![],
            },
        ])
    }

    #[test]
    fn hide_empty_hides_and_toggle_reveals_zero_issue_repos() {
        let mut app = app_with_empty_repo();
        // Default: hidden — only alpha's header + issue.
        assert_eq!(app.rows.len(), 2);

        app.toggle_hide_empty();
        assert_eq!(app.rows.len(), 3); // + empty-repo header
        assert!(matches!(app.rows[2], Row::RepoHeader { repo_idx: 1 }));
        assert_eq!(app.repo_visible_count(1), 0);

        app.toggle_hide_empty();
        assert_eq!(app.rows.len(), 2);
    }

    #[test]
    fn hide_empty_off_also_reveals_filtered_to_zero_groups() {
        let mut app = two_repo_app();
        app.filters.text = "docs".into(); // matches only beta's issue
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 2); // beta header + its issue

        app.toggle_hide_empty();
        // alpha reappears as an empty group under the same rule.
        assert!(
            app.rows
                .iter()
                .any(|r| matches!(r, Row::RepoHeader { repo_idx: 0 }))
        );
        assert_eq!(app.repo_visible_count(0), 0);
    }

    #[test]
    fn clear_filters_restores_config_default_not_false() {
        let mut app = app_with_empty_repo();
        app.set_hide_empty_default(false); // config says show empties
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 3);
        assert!(!app.filters_active(), "config default is not 'active'");

        app.toggle_hide_empty(); // user hides them this session
        assert!(app.filters_active());

        app.clear_filters();
        app.rebuild_rows();
        assert!(!app.filters.hide_empty); // back to config default
        assert!(!app.filters_active());
        assert_eq!(app.rows.len(), 3);
    }

    #[test]
    fn switch_org_restores_hide_empty_default() {
        let mut app = app_with_empty_repo();
        app.toggle_hide_empty();
        assert!(!app.filters.hide_empty);
        app.switch_org("other".into());
        assert!(app.filters.hide_empty); // default true restored
    }

    #[test]
    fn filters_active_only_on_hide_empty_deviation() {
        let mut app = two_repo_app();
        assert!(!app.filters_active());
        app.toggle_hide_empty();
        assert!(app.filters_active());
        app.toggle_hide_empty();
        assert!(!app.filters_active());
    }

    #[test]
    fn hide_empty_row_shows_yes_no_in_filter_menu() {
        let mut app = two_repo_app();
        assert_eq!(app.current_filter_value(FILTER_HIDE_EMPTY_IDX), "yes");
        app.toggle_hide_empty();
        assert_eq!(app.current_filter_value(FILTER_HIDE_EMPTY_IDX), "no");
    }

    #[test]
    fn auto_refresh_blocked_in_form_modes() {
        let mut app = two_repo_app();
        assert!(app.should_auto_refresh());
        for mode in [
            Mode::IssueForm,
            Mode::IssueFormSelect(4),
            Mode::IssueFormMulti(2),
            Mode::IssueFormBody,
            Mode::CommentEditor,
        ] {
            app.mode = mode;
            assert!(!app.should_auto_refresh(), "{mode:?} must block refresh");
        }
    }

    #[test]
    fn auto_refresh_gated_by_loading_rate_limit_and_mode() {
        let mut app = two_repo_app(); // set_data cleared `loading`
        assert!(app.should_auto_refresh());

        app.loading = true;
        assert!(!app.should_auto_refresh());
        app.loading = false;

        app.rate_limit_error = Some("rate limited".into());
        assert!(!app.should_auto_refresh());
        app.rate_limit_error = None;

        app.mode = Mode::Input(InputKind::Search);
        assert!(!app.should_auto_refresh());
        app.mode = Mode::ConfirmState;
        assert!(!app.should_auto_refresh());
        app.mode = Mode::Help;
        assert!(app.should_auto_refresh());
        app.mode = Mode::Normal;
        assert!(app.should_auto_refresh());
    }

    fn sample_pr_summary(pr: PrRef) -> PrSummary {
        PrSummary {
            pr,
            title: "t".into(),
            body: String::new(),
            state: crate::provider::types::PrState::Open,
            is_draft: false,
            base_ref: "main".into(),
            head_ref: "feature".into(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            comment_count: 0,
            review_thread_count: 0,
            reviews: Default::default(),
            checks: Default::default(),
            pr_runs: vec![],
            default_branch_name: "main".into(),
            default_branch_runs: vec![],
        }
    }

    #[test]
    fn collect_pr_links_scans_body_then_comments_in_order() {
        let mut app = two_repo_app();
        app.selected = 1; // first issue in alpha
        {
            let issue = &mut app.repos[0].issues[0];
            issue.body = "see https://github.com/o/r/pull/1".into();
        }
        app.detail_comments = Some(vec![Comment {
            id: "c1".into(),
            author: "x".into(),
            created_at: Utc::now(),
            body: "also https://github.com/o/r2/pull/2".into(),
        }]);
        let links = app.collect_pr_links();
        assert_eq!(
            links,
            vec![
                PrRef {
                    owner: "o".into(),
                    repo: "r".into(),
                    number: 1
                },
                PrRef {
                    owner: "o".into(),
                    repo: "r2".into(),
                    number: 2
                },
            ]
        );
    }

    #[test]
    fn open_pr_summary_sets_target_and_loading_state() {
        let mut app = two_repo_app();
        let pr = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        };
        app.open_pr_summary(pr.clone());
        assert_eq!(app.mode, Mode::PrSummary);
        assert_eq!(app.pr_target, Some(pr));
        assert!(app.pr_summary.is_none());
    }

    #[test]
    fn open_pr_picker_populates_options_from_links() {
        let mut app = two_repo_app();
        let links = vec![
            PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 1,
            },
            PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 2,
            },
        ];
        app.open_pr_picker(links);
        assert_eq!(app.mode, Mode::PrPicker);
        assert_eq!(app.select_options, vec!["o/r#1", "o/r#2"]);
    }

    #[test]
    fn set_pr_summary_applies_only_to_current_target() {
        let mut app = two_repo_app();
        let pr1 = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        };
        let pr2 = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 2,
        };
        app.open_pr_summary(pr1.clone());
        // A response for a different PR (the popup retargeted before this
        // landed) must not overwrite the current summary.
        app.set_pr_summary(&pr2, Ok(sample_pr_summary(pr2.clone())));
        assert!(app.pr_summary.is_none());

        app.set_pr_summary(&pr1, Ok(sample_pr_summary(pr1.clone())));
        assert!(app.pr_summary.is_some());
    }

    #[test]
    fn close_detail_clears_pr_state() {
        let mut app = two_repo_app();
        app.selected = 1;
        app.open_detail();
        let pr = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        };
        app.open_pr_summary(pr.clone());
        app.set_pr_summary(&pr.clone(), Ok(sample_pr_summary(pr)));
        app.close_detail();
        assert!(app.pr_target.is_none());
        assert!(app.pr_summary.is_none());
    }
}
