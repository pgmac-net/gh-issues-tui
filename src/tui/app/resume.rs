//! Taking a snapshot of the layout, and resuming from one (#236).
//!
//! Most of a layout (filters, sort, collapse) applies at once, before the
//! first fetch, so a restored filter also scopes that fetch (#230). The part
//! that needs data (which issue was selected, whether the detail pane was
//! open) waits in [`Resume`] and is applied when the first load lands. Until
//! then a snapshot reports the pending values, so a quick quit or an early
//! debounce can never overwrite a saved selection with "nothing".

use std::time::Instant;

use super::prelude::*;
use super::repo_runs::Grouping;
use crate::tui::layout_state::{Layout, SavedFilters, Selected, VERSION};

/// The part of a saved layout that needs the data to have loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub selected: Option<Selected>,
    pub list_top: usize,
    pub detail_open: bool,
    pub detail_focused: bool,
    pub runs_tab: bool,
}

fn sorted(set: &std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

impl App {
    /// A snapshot of the layout worth resuming. Sorted, so two equal states
    /// compare equal and a change is a real one.
    pub fn layout(&self, provider: &str) -> Layout {
        let f = &self.filters;
        let (selected, list_top, detail_open, detail_focused, runs_tab) = match &self.resume {
            // Data has not landed yet: report what is waiting to be applied.
            Some(r) => (
                r.selected.clone(),
                r.list_top,
                r.detail_open,
                r.detail_focused,
                r.runs_tab,
            ),
            None => (
                self.selected_for_layout(),
                self.list_top.get(),
                self.detail.open,
                self.detail.open && self.focus == Focus::Detail,
                self.resume_runs || self.runs_showing(),
            ),
        };
        Layout {
            version: VERSION,
            provider: provider.to_string(),
            filters: SavedFilters {
                text: f.text.clone(),
                repo: f.repo.clone(),
                assignee: f.assignee.clone(),
                author: f.author.clone(),
                priority: f.priority.clone(),
                status: f.status.clone(),
                created_after: f.created_after,
                created_before: f.created_before,
                updated_after: f.updated_after,
                updated_before: f.updated_before,
                closed_after: f.closed_after,
                closed_before: f.closed_before,
                hide_empty: f.hide_empty,
            },
            state_filter: self.state_filter.label().to_string(),
            sort_key: self.sort_key.label().to_string(),
            sort_desc: self.sort_desc,
            collapsed: sorted(&self.collapsed),
            seen_repos: sorted(&self.seen_repos),
            collapsed_issues: sorted(&self.collapsed_issues),
            folded_comments: sorted(&self.folded_comments),
            selected,
            list_top,
            detail_open,
            detail_focused,
            runs_tab,
            runs_group: self.repo_runs.group.label().to_string(),
        }
    }

    /// The selected row as a layout names it: an issue (loaded or a stub) by
    /// id, a repo header by name.
    fn selected_for_layout(&self) -> Option<Selected> {
        match self.rows.get(self.selected)? {
            Row::RepoHeader { repo_idx } => {
                Some(Selected::Repo(self.repos.get(*repo_idx)?.repo.clone()))
            }
            row => self
                .row_issue_id(row)
                .map(|id| Selected::Issue(id.to_string())),
        }
    }

    /// Resume from a saved layout. Called once at startup, before the first
    /// fetch. `keep_repo` leaves the repo filter alone: a repo detected from
    /// the clone you started in is a startup input and beats what was saved.
    pub fn apply_layout(&mut self, l: &Layout, keep_repo: bool) {
        let s = &l.filters;
        let f = &mut self.filters;
        f.text = s.text.clone();
        if !keep_repo {
            f.repo = s.repo.clone();
        }
        f.assignee = s.assignee.clone();
        f.author = s.author.clone();
        f.priority = s.priority.clone();
        f.status = s.status.clone();
        f.created_after = s.created_after;
        f.created_before = s.created_before;
        f.updated_after = s.updated_after;
        f.updated_before = s.updated_before;
        f.closed_after = s.closed_after;
        f.closed_before = s.closed_before;
        f.hide_empty = s.hide_empty;
        self.state_filter = StateFilter::from_label(&l.state_filter).unwrap_or(StateFilter::Open);
        self.sort_key = SortKey::from_label(&l.sort_key).unwrap_or(SortKey::Updated);
        self.sort_desc = l.sort_desc;
        self.collapsed = l.collapsed.iter().cloned().collect();
        self.seen_repos = l.seen_repos.iter().cloned().collect();
        self.collapsed_issues = l.collapsed_issues.iter().cloned().collect();
        self.folded_comments = l.folded_comments.iter().cloned().collect();
        // Needs no data, so it applies at once (#266).
        // A configured grouping beats the saved one (#271).
        self.repo_runs.group = self
            .repo_runs
            .default_group
            .or_else(|| Grouping::from_label(&l.runs_group))
            .unwrap_or_default();
        self.resume = Some(Resume {
            selected: l.selected.clone(),
            list_top: l.list_top,
            detail_open: l.detail_open,
            detail_focused: l.detail_focused,
            runs_tab: l.runs_tab,
        });
    }

    /// Apply the waiting selection and detail pane, once data has landed. Runs
    /// inside the `nav()` closure of the load, so the detail pane follows the
    /// selection like any other change. A selection that is gone (closed,
    /// moved) simply leaves the list where `set_data` put it.
    pub(super) fn apply_resume(&mut self) {
        let Some(r) = self.resume.take() else { return };
        match &r.selected {
            Some(Selected::Issue(id)) => self.reselect(Some(id.clone())),
            Some(Selected::Repo(name)) => {
                if let Some(idx) = self.rows.iter().position(|row| {
                    matches!(row, Row::RepoHeader { repo_idx }
                        if self.repos.get(*repo_idx).is_some_and(|x| &x.repo == name))
                }) {
                    self.selected = idx;
                }
            }
            None => {}
        }
        self.list_top.set(r.list_top);
        // The pane only makes sense with an issue under the cursor, or a repo
        // header with CI behind it: the repo workflows view (#266).
        let pane = self.selected_issue().is_some()
            || (self.selected_is_repo_header() && self.runs.supported);
        if r.detail_open && pane {
            self.open_detail();
            if !r.detail_focused {
                self.focus = Focus::List;
            }
            self.resume_runs = r.runs_tab;
        }
    }

    /// Switch the lower region to the runs tab if the saved layout had it
    /// showing. Needs the clock, so the event loop calls it after the load.
    pub fn resume_runs_tab(&mut self, now: Instant) {
        if std::mem::take(&mut self.resume_runs)
            && self.runs.supported
            && self.detail.open
            && !self.runs_showing()
            && self.selected_issue().is_some()
        {
            self.toggle_runs_tab(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::RepoIssues;

    fn app() -> App {
        App::new("org".into(), None, false, false, String::new())
    }

    fn issue(n: u64) -> Issue {
        Issue {
            updated_at: "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
                + chrono::TimeDelta::seconds(n as i64),
            ..Issue::fixture(&format!("I_{n}"), n)
        }
    }

    fn data() -> Vec<RepoIssues> {
        vec![
            RepoIssues {
                repo: "api".into(),
                repo_url: String::new(),
                issues: (1..=3).map(issue).collect(),
            },
            RepoIssues {
                repo: "docs".into(),
                repo_url: String::new(),
                issues: vec![issue(9)],
            },
        ]
    }

    /// An app that has loaded, with every saved field moved off its default.
    fn moved() -> App {
        let mut a = app();
        a.set_data(data());
        a.filters.text = "t".into();
        a.filters.assignee = "pgmac".into();
        a.filters.priority = vec!["high".into()];
        a.filters.updated_after = NaiveDate::from_ymd_opt(2026, 10, 1);
        a.filters.hide_empty = false;
        a.state_filter = StateFilter::All;
        a.sort_key = SortKey::Priority;
        a.sort_desc = false;
        a.collapsed.insert("docs".into());
        a.collapsed_issues.insert("I_1".into());
        a.folded_comments.insert("C_1".into());
        a.rebuild_rows();
        a
    }

    #[test]
    fn a_snapshot_applied_to_a_fresh_app_snapshots_the_same() {
        let a = moved();
        let saved = a.layout("github");
        let json = serde_json::to_string(&saved).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();

        let mut b = app();
        b.apply_layout(&back, false);
        // Fields that apply at once come back immediately; the rest wait.
        let mut want = saved.clone();
        want.selected = back.selected.clone();
        assert_eq!(b.layout("github"), want);
        assert_eq!(b.filters.assignee, "pgmac");
        assert_eq!(b.state_filter, StateFilter::All);
        assert_eq!(b.sort_key, SortKey::Priority);
        assert!(b.collapsed.contains("docs") && b.collapsed_issues.contains("I_1"));
    }

    #[test]
    fn a_startup_repo_beats_the_saved_repo_filter() {
        let mut a = moved();
        a.filters.repo = "docs".into();
        let saved = a.layout("github");

        let mut keep = App::new(
            "org".into(),
            Some("api".into()),
            false,
            false,
            String::new(),
        );
        keep.apply_layout(&saved, true);
        assert_eq!(keep.filters.repo, "api", "the clone's repo wins");

        let mut take = app();
        take.apply_layout(&saved, false);
        assert_eq!(take.filters.repo, "docs", "no clone: the saved one");
    }

    #[test]
    fn the_selection_and_detail_pane_wait_for_data() {
        let mut a = app();
        a.set_data(data());
        a.selected = a
            .rows
            .iter()
            .position(|r| matches!(r, Row::Issue { .. }) && a.row_issue(r).unwrap().number == 2)
            .unwrap();
        a.open_detail();
        let saved = a.layout("github");
        assert_eq!(saved.selected, Some(Selected::Issue("I_2".into())));
        assert!(saved.detail_open && saved.detail_focused);

        let mut b = app();
        b.apply_layout(&saved, false);
        assert!(!b.detail.open, "nothing to show before data lands");
        assert_eq!(
            b.layout("github").selected,
            saved.selected,
            "an early snapshot still reports the saved selection"
        );

        b.set_data(data());
        assert_eq!(b.selected_issue().map(|i| i.number), Some(2));
        assert!(b.detail.open && b.focus == Focus::Detail);
        assert!(b.resume.is_none(), "applied once");
    }

    #[test]
    fn a_header_selection_resumes_on_that_header() {
        let mut a = app();
        a.set_data(data());
        a.selected = a
            .rows
            .iter()
            .rposition(|r| matches!(r, Row::RepoHeader { .. }))
            .unwrap();
        let saved = a.layout("github");
        assert_eq!(saved.selected, Some(Selected::Repo("docs".into())));

        let mut b = app();
        b.apply_layout(&saved, false);
        b.set_data(data());
        assert_eq!(b.selected_repo().map(|r| r.repo.as_str()), Some("docs"));
        assert!(b.selected_is_header());
    }

    #[test]
    fn a_selection_that_is_gone_falls_back_quietly() {
        let mut a = app();
        a.set_data(data());
        let mut saved = a.layout("github");
        saved.selected = Some(Selected::Issue("I_404".into()));
        saved.detail_open = true;

        let mut b = app();
        b.apply_layout(&saved, false);
        b.set_data(data());
        assert_eq!(b.selected, 0, "top of the list");
        assert!(!b.detail.open, "no issue to show, so no pane");
        assert!(b.status.is_none(), "no complaint");
    }

    #[test]
    fn a_resumed_repo_keeps_the_expansion_it_had() {
        // `default_collapsed` folds first-seen repos; a repo you expanded last
        // time was seen, so it must stay open.
        let mut a = App::new("org".into(), None, false, true, String::new());
        a.set_data(data());
        a.collapsed.remove("api");
        a.rebuild_rows();
        let saved = a.layout("github");

        let mut b = App::new("org".into(), None, false, true, String::new());
        b.apply_layout(&saved, false);
        b.set_data(data());
        assert!(!b.collapsed.contains("api"));
        assert!(b.collapsed.contains("docs"));
    }

    #[test]
    fn unknown_labels_fall_back_to_the_defaults() {
        let mut saved = app().layout("github");
        saved.state_filter = "bogus".into();
        saved.sort_key = "bogus".into();
        let mut b = app();
        b.apply_layout(&saved, false);
        assert_eq!(b.state_filter, StateFilter::Open);
        assert_eq!(b.sort_key, SortKey::Updated);
    }

    #[test]
    fn the_runs_tab_resumes_after_the_load() {
        let mut a = app();
        a.runs.supported = true;
        a.set_data(data());
        a.selected = 1;
        a.open_detail();
        a.toggle_runs_tab(Instant::now());
        let saved = a.layout("github");
        assert!(saved.runs_tab);

        let mut b = app();
        b.runs.supported = true;
        b.apply_layout(&saved, false);
        b.set_data(data());
        assert!(!b.runs_showing(), "needs the clock, so it waits");
        assert!(b.layout("github").runs_tab, "and is still reported");
        b.resume_runs_tab(Instant::now());
        assert!(b.runs_showing());
    }

    #[test]
    fn the_repo_workflows_view_resumes_with_its_grouping() {
        let mut a = app();
        a.runs.supported = true;
        a.set_data(data());
        a.selected = a
            .rows
            .iter()
            .position(|r| matches!(r, Row::RepoHeader { .. }))
            .unwrap();
        a.open_detail();
        a.cycle_repo_grouping();
        assert!(a.repo_view_showing());
        let saved = a.layout("github");
        assert_eq!(saved.runs_group, "workflow");

        let mut b = app();
        b.runs.supported = true;
        b.apply_layout(&saved, false);
        assert_eq!(b.repo_runs.group, Grouping::Workflow, "applies before data");
        b.set_data(data());
        assert!(b.repo_view_showing(), "the header and the pane come back");
        assert_eq!(b.focus, Focus::Detail);
    }

    #[test]
    fn a_configured_grouping_beats_the_saved_one() {
        let mut saved = app().layout("github");
        saved.runs_group = "workflow".into();
        let mut b = app();
        b.repo_runs.default_group = Some(Grouping::Flat);
        b.apply_layout(&saved, false);
        assert_eq!(b.repo_runs.group, Grouping::Flat);
    }

    #[test]
    fn a_layout_without_a_grouping_takes_the_default() {
        let mut saved = app().layout("github");
        saved.runs_group = String::new();
        let mut b = app();
        b.repo_runs.group = Grouping::Branch;
        b.apply_layout(&saved, false);
        assert_eq!(b.repo_runs.group, Grouping::Status);
    }
}
