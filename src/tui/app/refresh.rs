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
    /// A context row refreshes its issue when it is loaded (#234). A stub has
    /// nothing loaded to patch, so `r` there refreshes the group it is drawn
    /// in, which is where the child that names it lives.
    pub fn refresh_target(&self) -> RefreshTarget {
        match self.rows.get(self.selected) {
            Some(Row::Issue { .. } | Row::Context { .. }) => match self.selected_issue() {
                Some(i) => RefreshTarget::Issue {
                    id: i.id.clone(),
                    number: i.number,
                },
                None => self.selected_repo().map_or(RefreshTarget::Filtered, |r| {
                    RefreshTarget::Repo(r.repo.clone())
                }),
            },
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
    ///
    /// Before anything has loaded there are no names to match against, and
    /// the only repo filter that can exist is the one detected from the
    /// clone you started in (#230): that one is trusted as exact. If the repo
    /// turns out not to exist, the group comes back gone, the filter no
    /// longer matches exactly, and the auto-widen falls back to the org.
    pub fn filtered_target(&self) -> FetchTarget {
        if self.filters.repo.is_empty() {
            return FetchTarget::Org;
        }
        if self.loaded.is_none() && self.repos.is_empty() {
            return FetchTarget::Repo(self.filters.repo.clone());
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
        // Gone is gone: a pin must not hold it (#249).
        self.pins.release(id);
        if removed {
            self.data_gen += 1;
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
        self.data_gen += 1;
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
        if incoming.is_none() {
            // The repo itself is gone: nothing in it can be pinned back.
            for issue in loaded.iter().flat_map(|r| &r.issues) {
                self.pins.release(&issue.id);
            }
        }
        if let Some(fresh) = incoming {
            self.vocab.absorb_repos(std::slice::from_ref(&fresh));
            let merged =
                keep_newer_loaded(loaded.into_iter().collect(), vec![fresh], &mut self.pins)
                    .pop()
                    .expect("one repo in, one repo out");
            let name = merged.repo.clone();
            // Keep the org query's name order wherever the repo lands.
            let at = slot.unwrap_or_else(|| {
                self.repos
                    .iter()
                    .position(|r| r.repo.to_lowercase() > name.to_lowercase())
                    .unwrap_or(self.repos.len())
            });
            self.repos.insert(at, merged);
            // A first-seen repo takes `default_collapsed`, except when it is
            // the only group the filters leave visible — the same rule as
            // `set_data`. Startup inside a clone lands here (#230), and one
            // folded header is all it would otherwise show.
            if self.seen_repos.insert(name.clone())
                && self.default_collapsed
                && self.single_visible_repo().as_deref() != Some(name.as_str())
            {
                self.collapsed.insert(name);
            }
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
        // The first load of a clone-scoped start lands here, not in
        // `set_data`, so a saved selection resumes from either (#236).
        self.apply_resume();
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
            title: format!("t{number}"),
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            updated_at: updated.parse().unwrap(),
            ..Issue::fixture(id, number)
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

    /// #230: started inside a clone, the detected repo is fetched alone.
    #[test]
    fn before_the_first_load_the_detected_repo_is_the_target() {
        let app = App::new(
            "org".into(),
            Some("gh-issues-tui".into()),
            false,
            false,
            String::new(),
        );
        assert_eq!(
            app.filtered_target(),
            FetchTarget::Repo("gh-issues-tui".into())
        );
    }

    /// #230, found live: a clone-scoped start lands through `replace_repo`,
    /// which collapsed the lone group under `default_collapsed`.
    #[test]
    fn a_scoped_first_load_leaves_the_only_group_expanded() {
        let mut app = App::new(
            "org".into(),
            Some("alpha".into()),
            false,
            true,
            String::new(),
        );
        app.replace_repo("alpha", Some(repo("alpha", vec![issue("I_1", 1, T1)])));
        assert!(!app.collapsed.contains("alpha"));
        assert_eq!(app.rows.len(), 2, "header and its issue");

        // A second first-seen group is not alone, so it still folds.
        app.filters.repo.clear();
        app.replace_repo("beta", Some(repo("beta", vec![issue("I_2", 2, T1)])));
        assert!(app.collapsed.contains("beta"));
    }

    /// #230: the detected repo came back gone, so nothing matches it exactly
    /// any more and the next fetch is the org.
    #[test]
    fn a_detected_repo_that_turned_out_gone_falls_back_to_the_org() {
        let mut app = App::new(
            "org".into(),
            Some("nope".into()),
            false,
            false,
            String::new(),
        );
        app.begin_fetch(app.wanted());
        app.finish_fetch(true);
        app.replace_repo("nope", None);
        assert_eq!(app.filtered_target(), FetchTarget::Org);
        app.loading = false;
        assert!(app.widened(), "the org is wider than the repo that loaded");
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
