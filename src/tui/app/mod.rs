//! All application state and the pure logic over it.
//!
//! No I/O and no screen geometry live here — geometry is `tui::layout`'s and
//! every fetch goes through `tui::event`. Split by concern: the filter and
//! sort model, the text editors, the new-issue form, the mode enums, the
//! picker, and the detail-pane and PR-summary state.

mod detail;
mod editor;
mod filter_input;
mod filters;
mod form;
mod mode;
mod picker;
mod pr;
mod rows;

#[cfg(test)]
mod tests;

pub use editor::*;
pub use filters::*;
pub use form::*;
pub use mode::*;

use prelude::*;

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
    /// Which region of the detail pane has focus (body or a comment card).
    /// Tab/Shift+Tab move it; `j/k` scroll the selected region; `e` edits it.
    pub detail_sel: DetailSel,
    /// Visual-row scroll offset within the body region.
    pub body_scroll: u16,
    /// Visual-row scroll offset into the stacked comments paragraph. Selecting
    /// a comment snaps this to that comment's top; `j/k` then scroll within
    /// the selected comment's own extent.
    pub comments_scroll: u16,
    /// Candidate PR links, populated when more than one is found (`Mode::PrPicker`).
    pub pr_links: Vec<PrRef>,
    /// The PR currently being fetched or shown; guards against a stale
    /// `PrSummary` response landing after the target moved on.
    pub pr_target: Option<PrRef>,
    /// `None` while the summary fetch for `pr_target` is in flight.
    pub pr_summary: Option<Result<PrSummary, String>>,
    pub pr_scroll: u16,
    /// Index into `pr_targets()` — the PR summary's open-able row (PR header,
    /// a check, or a workflow run) that `o`/Enter will open and that the
    /// popup highlights. `Tab`/`Shift+Tab` move it.
    pub pr_sel: usize,
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
            detail_sel: DetailSel::Body,
            body_scroll: 0,
            comments_scroll: 0,
            pr_links: Vec::new(),
            pr_target: None,
            pr_summary: None,
            pr_scroll: 0,
            pr_sel: 0,
            should_quit: false,
        }
    }

    /// Open an option picker: set its options and initial highlight, and
    /// Whether the auto-refresh ticker may fire now: not while a fetch is
    /// in flight, rate-limited, or anything interactive is open (only the
    /// passive Normal and Help modes qualify).
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
        self.reset_detail_scroll();
        self.clear_pr_state();
        self.loading = true;
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
}
/// Items the state submodules share. One place for what was a single import
/// block at the top of the old `app.rs`.
pub(crate) mod prelude {
    pub use chrono::{DateTime, NaiveDate, Utc};

    pub use crate::provider::types::{
        Comment, FormOptions, IdName, Issue, IssueState, NewIssueParams, PrRef, PrSummary,
        RateLimitData, RepoIssues, RepoLabel, parse_pr_links, priority_value, priority_value_rank,
    };

    pub use super::editor::*;
    pub use super::filters::*;
    pub use super::form::*;
    pub use super::mode::*;

    pub use super::App;
}
