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
mod harness;
mod mode;
mod picker;
mod pr;
mod rows;

#[cfg(test)]
mod tests;

pub use detail::DetailState;
pub use editor::*;
pub use filters::*;
pub use form::*;
pub use harness::{HarnessState, LaunchAction, SessionId, SessionMeta, SessionStatus};
pub use mode::*;
pub use picker::PickerState;
pub use pr::PrState;

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
    /// The detail pane (right split) and its two scrolling regions.
    pub detail: DetailState,
    pub mode: Mode,
    pub input: InputState,
    pub filter_menu_idx: usize,
    /// The option-picker popup's state, for every mode that shows one.
    pub picker: PickerState,
    /// The new-issue form, present while it is open.
    pub issue_form: Option<IssueForm>,
    /// The inline comment/description editor's session state.
    pub editor: EditorState,
    /// Which button has keys in the `Mode::ConfirmState` popup; reset to
    /// `No` each time the popup opens.
    pub confirm_choice: ConfirmChoice,
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
    /// The PR summary popup and the links that feed it.
    pub pr: PrState,
    /// Comment threads already fetched this refresh cycle, keyed by issue id.
    ///
    /// Navigating with the detail pane open used to spawn one request per row
    /// (#107); this makes revisiting a row free. Cleared wholesale by
    /// `set_data` and `switch_org` — a refetch can reveal comments added
    /// elsewhere, and a stale thread is worse than a cheap refetch.
    pub comment_cache: HashMap<String, Vec<Comment>>,
    /// Coding-harness sessions (#23) — metadata only; the PTYs themselves
    /// are owned by the event loop. Deliberately *not* reset by
    /// `switch_org`: an agent working a ticket is unaffected by the list
    /// being pointed at another owner.
    pub harness: HarnessState,
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
            detail: DetailState::default(),
            mode: Mode::Normal,
            input: InputState::default(),
            filter_menu_idx: 0,
            picker: PickerState::default(),
            issue_form: None,
            editor: EditorState::default(),
            confirm_choice: ConfirmChoice::No,
            calendar_cursor: Utc::now().date_naive(),
            loading: true,
            auto_refreshing: false,
            include_closed,
            status: None,
            rate_limit: None,
            rate_limit_error: None,
            comment_cache: HashMap::new(),
            pr: PrState::default(),
            harness: HarnessState::default(),
            should_quit: false,
        }
    }

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
        // Fresh data can carry comments added since the last fetch, so the
        // cached threads are no longer trustworthy.
        self.comment_cache.clear();
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
        self.comment_cache.clear();
        self.rows.clear();
        self.collapsed.clear();
        self.seen_repos.clear();
        self.clear_filters();
        self.state_filter = StateFilter::Open;
        self.selected = 0;
        self.focus = Focus::List;
        self.detail.open = false;
        self.detail.comments = None;
        self.detail.reset_scroll();
        self.pr = PrState::default();
        self.loading = true;
    }

    /// Tab / Shift+Tab: move focus to the other pane. With two panes the
    /// direction doesn't matter; no-op when the split is closed.
    pub fn cycle_focus(&mut self) {
        if self.detail.open {
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
    pub use std::collections::HashMap;

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

    pub use super::pr::PrState;
}
