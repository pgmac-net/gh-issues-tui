//! Granular refresh (#229): which slice of the data a refresh asks for, and
//! folding a one-issue or one-repo answer back into the loaded list.
//!
//! `r` refreshes what is selected (an issue, or a repo group); `Ctrl+R` and
//! the auto-refresh ticker refresh the *filtered scope*: the one repo an exact
//! repo filter names, otherwise the whole org. A one-issue answer goes through
//! [`App::patch_issue`] like a mutation's, so `keep_newer_loaded` still holds.

use super::patch::keep_newer_loaded;
use super::prelude::*;

/// What `r` refreshes, from the selected row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshTarget {
    Issue {
        id: String,
        number: u64,
    },
    Repo(String),
    /// Nothing selected (an empty list): fall back to the filtered scope.
    Filtered,
}

/// What `Ctrl+R` and auto-refresh fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchTarget {
    Org,
    /// The repo an exact repo filter names, spelled as loaded.
    Repo(String),
}

impl App {
    pub fn refresh_target(&self) -> RefreshTarget {
        match self.rows.get(self.selected) {
            Some(Row::Issue { .. }) => {
                self.selected_issue()
                    .map_or(RefreshTarget::Filtered, |i| RefreshTarget::Issue {
                        id: i.id.clone(),
                        number: i.number,
                    })
            }
            Some(Row::RepoHeader { .. }) => {
                self.selected_repo().map_or(RefreshTarget::Filtered, |r| {
                    RefreshTarget::Repo(r.repo.clone())
                })
            }
            None => RefreshTarget::Filtered,
        }
    }

    /// The filtered scope: an exact repo filter narrows the fetch to that
    /// repo; anything else (no filter, a substring) fetches the org.
    pub fn filtered_target(&self) -> FetchTarget {
        if self.filters.repo.is_empty() {
            return FetchTarget::Org;
        }
        self.repos
            .iter()
            .find(|r| r.repo.eq_ignore_ascii_case(&self.filters.repo))
            .map_or(FetchTarget::Org, |r| FetchTarget::Repo(r.repo.clone()))
    }

    /// Drop the issue with `id` — a refresh found it gone. Returns `false`
    /// when it was not loaded. Callers route through `nav()`.
    pub fn remove_issue(&mut self, id: &str) -> bool {
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        let mut removed = false;
        for repo in &mut self.repos {
            let before = repo.issues.len();
            repo.issues.retain(|i| i.id != id);
            removed |= repo.issues.len() != before;
        }
        if removed {
            self.invalidate_comments(id);
            self.rebuild_rows();
            self.reselect(prev_selected);
        }
        removed
    }

    /// Replace one repo's issues with a fresh fetch, or drop the group when
    /// `incoming` is `None` (gone, archived, issues disabled). Other repos
    /// are untouched. Within the repo the refetch decides membership and
    /// `keep_newer_loaded` keeps any newer patched copy, exactly as
    /// `set_data` does for the whole org.
    pub fn replace_repo(&mut self, name: &str, incoming: Option<RepoIssues>) {
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        let prev_header = match self.rows.get(self.selected) {
            Some(Row::RepoHeader { .. }) => self.selected_repo().map(|r| r.repo.clone()),
            _ => None,
        };
        let slot = self.repos.iter().position(|r| r.repo == name);
        let loaded = slot.map(|i| self.repos.remove(i));
        // Threads and judgements for this repo's issues may be stale now —
        // the same reason `set_data` clears them org-wide.
        for issue in loaded.iter().flat_map(|r| &r.issues) {
            self.invalidate_comments(&issue.id);
        }
        if let Some(fresh) = incoming {
            let merged = keep_newer_loaded(loaded.into_iter().collect(), vec![fresh])
                .pop()
                .expect("one repo in, one repo out");
            if self.seen_repos.insert(merged.repo.clone()) && self.default_collapsed {
                self.collapsed.insert(merged.repo.clone());
            }
            // Keep the org query's name order wherever the repo lands.
            let at = slot.unwrap_or_else(|| {
                self.repos
                    .iter()
                    .position(|r| r.repo.to_lowercase() > merged.repo.to_lowercase())
                    .unwrap_or(self.repos.len())
            });
            self.repos.insert(at, merged);
        }
        self.stamp_label_ranks();
        self.rebuild_rows();
        self.reselect(prev_selected);
        if let Some(repo) = prev_header
            && let Some(idx) = self.rows.iter().position(|row| {
                matches!(row, Row::RepoHeader { repo_idx }
                    if self.repos.get(*repo_idx).is_some_and(|r| r.repo == repo))
            })
        {
            self.selected = idx;
        }
    }

    /// Issue count of a loaded repo, for the status line.
    pub fn repo_issue_count(&self, name: &str) -> usize {
        self.repos
            .iter()
            .find(|r| r.repo == name)
            .map_or(0, |r| r.issues.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, number: u64, updated: &str) -> Issue {
        Issue {
            id: id.into(),
            number,
            title: format!("t{number}"),
            body: String::new(),
            state: IssueState::Open,
            url: "u".into(),
            author: "a".into(),
            assignees: vec![],
            labels: vec![],
            comment_count: 0,
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            updated_at: updated.parse().unwrap(),
            closed_at: None,
        }
    }

    fn repo(name: &str, issues: Vec<Issue>) -> RepoIssues {
        RepoIssues {
            repo: name.into(),
            repo_url: format!("https://x/{name}"),
            issues,
        }
    }

    const T1: &str = "2026-01-01T00:00:00Z";
    const T2: &str = "2026-02-01T00:00:00Z";

    /// Two expanded repos: alpha (#1, #2) and gamma (#3).
    fn app() -> App {
        let mut app = App::new("org".into(), None, false, false, "{repo}#{number}".into());
        app.set_data(vec![
            repo("alpha", vec![issue("I_1", 1, T1), issue("I_2", 2, T1)]),
            repo("gamma", vec![issue("I_3", 3, T1)]),
        ]);
        app
    }

    fn select_issue(app: &mut App, id: &str) {
        app.reselect(Some(id.into()));
        assert_eq!(app.selected_issue().map(|i| i.id.as_str()), Some(id));
    }

    fn header_row(app: &App, name: &str) -> usize {
        app.rows
            .iter()
            .position(|row| {
                matches!(row, Row::RepoHeader { repo_idx } if app.repos[*repo_idx].repo == name)
            })
            .expect("header row")
    }

    #[test]
    fn refresh_target_follows_the_selected_row() {
        let mut app = app();
        select_issue(&mut app, "I_2");
        assert_eq!(
            app.refresh_target(),
            RefreshTarget::Issue {
                id: "I_2".into(),
                number: 2
            }
        );
        app.selected = header_row(&app, "gamma");
        assert_eq!(app.refresh_target(), RefreshTarget::Repo("gamma".into()));

        let empty = App::new("org".into(), None, false, false, String::new());
        assert_eq!(empty.refresh_target(), RefreshTarget::Filtered);
    }

    #[test]
    fn filtered_target_narrows_only_on_an_exact_repo_match() {
        let mut app = app();
        assert_eq!(app.filtered_target(), FetchTarget::Org);
        app.filters.repo = "ALPHA".into();
        assert_eq!(app.filtered_target(), FetchTarget::Repo("alpha".into()));
        app.filters.repo = "alp".into();
        assert_eq!(app.filtered_target(), FetchTarget::Org);
    }

    #[test]
    fn remove_issue_drops_it_and_selects_a_neighbour() {
        let mut app = app();
        select_issue(&mut app, "I_1");
        assert!(app.remove_issue("I_1"));
        assert!(app.repos[0].issues.iter().all(|i| i.id != "I_1"));
        assert!(app.selected_issue().is_some());
        assert!(!app.remove_issue("I_404"));
    }

    #[test]
    fn replace_repo_touches_only_that_repo() {
        let mut app = app();
        select_issue(&mut app, "I_3");
        app.replace_repo("alpha", Some(repo("alpha", vec![issue("I_9", 9, T2)])));
        let alpha: Vec<_> = app.repos[0].issues.iter().map(|i| i.number).collect();
        assert_eq!(alpha, vec![9], "membership is the refetch's to decide");
        assert_eq!(app.repos[1].issues[0].id, "I_3", "gamma untouched");
        assert_eq!(
            app.selected_issue().map(|i| i.id.as_str()),
            Some("I_3"),
            "selection follows the issue id"
        );
    }

    #[test]
    fn replace_repo_keeps_a_newer_patched_copy() {
        let mut app = app();
        app.repos[0].issues[0].updated_at = T2.parse().unwrap();
        app.repos[0].issues[0].title = "patched".into();
        app.replace_repo(
            "alpha",
            Some(repo(
                "alpha",
                vec![issue("I_1", 1, T1), issue("I_2", 2, T1)],
            )),
        );
        assert_eq!(app.repos[0].issues[0].title, "patched");
    }

    #[test]
    fn replace_repo_with_none_drops_the_group() {
        let mut app = app();
        app.replace_repo("gamma", None);
        assert_eq!(app.repos.len(), 1);
        assert_eq!(app.repos[0].repo, "alpha");
    }

    #[test]
    fn replace_repo_inserts_an_unloaded_repo_in_name_order() {
        let mut app = app();
        app.replace_repo("beta", Some(repo("beta", vec![issue("I_5", 5, T1)])));
        let names: Vec<_> = app.repos.iter().map(|r| r.repo.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn replace_repo_keeps_a_selected_header_selected() {
        let mut app = app();
        app.selected = header_row(&app, "gamma");
        // alpha grows by two rows above gamma's header.
        app.replace_repo(
            "alpha",
            Some(repo(
                "alpha",
                (1..=4).map(|n| issue(&format!("I_{n}"), n, T1)).collect(),
            )),
        );
        assert_eq!(app.selected, header_row(&app, "gamma"));
    }

    #[test]
    fn replace_repo_drops_cached_threads_for_its_issues() {
        let mut app = app();
        app.comment_cache.insert("I_1".into(), vec![]);
        app.comment_cache.insert("I_3".into(), vec![]);
        app.replace_repo("alpha", Some(repo("alpha", vec![issue("I_1", 1, T1)])));
        assert!(!app.comment_cache.contains_key("I_1"));
        assert!(app.comment_cache.contains_key("I_3"));
    }
}
