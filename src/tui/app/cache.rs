//! The list's side of the snapshot cache (#226, ADR 0010).
//!
//! The event loop does the I/O (`event/persist.rs`); this turns a snapshot
//! into list data and back, and says how old what is showing is.

use super::prelude::*;
use crate::cache::snapshot::Snapshot;

impl App {
    /// Draw a usable snapshot before the first fetch lands. It settles like
    /// any load, so a later fetch folds over it by `keep_newer_loaded`, and
    /// the loaded scope becomes the snapshot's until that fetch replaces it.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        // `set_data` ends a load, but a snapshot is not one: the startup
        // fetch is still to come, and the indicator says "refreshing…".
        let loading = self.loading;
        self.set_data(snapshot.repos);
        self.loading = loading;
        self.loaded = Some(snapshot.loaded);
        self.fetched_at = Some(snapshot.saved_at);
        self.cached = Some(snapshot.saved_at);
        self.cache_refresh_failed = false;
    }

    /// A filter widened past the loaded scope, and the snapshot covers it
    /// (#226): fold the snapshot in instead of fetching. Membership is the
    /// union, so nothing loaded since is lost, and `keep_newer_loaded` keeps
    /// the newer copy of every issue in both. The list is then as old as the
    /// snapshot, which the indicator says; `Ctrl+R` or auto-refresh replaces
    /// it.
    pub fn widen_from_snapshot(&mut self, snapshot: Snapshot) {
        let mut merged = snapshot.repos;
        for repo in &self.repos {
            match merged.iter_mut().find(|r| r.repo == repo.repo) {
                Some(have) => {
                    for issue in &repo.issues {
                        if !have.issues.iter().any(|i| i.id == issue.id) {
                            have.issues.push(issue.clone());
                        }
                    }
                }
                None => merged.push(repo.clone()),
            }
        }
        merged.sort_by_key(|r| r.repo.to_lowercase());
        let loading = self.loading;
        self.set_data(merged);
        self.loading = loading;
        self.loaded = Some(snapshot.loaded);
        let at = snapshot.saved_at;
        self.fetched_at = Some(self.fetched_at.map_or(at, |f| f.min(at)));
        self.cached = Some(at);
        self.cache_refresh_failed = false;
    }

    /// What the snapshot saver writes: everything loaded, with the scope it
    /// was fetched at and when. `None` before anything has loaded.
    pub fn snapshot(&self) -> Option<Snapshot> {
        if self.repos.is_empty() {
            return None;
        }
        Some(Snapshot::new(
            self.fetched_at?,
            self.loaded.clone()?,
            self.repos.clone(),
        ))
    }

    /// `cached 3h ago · refreshing…` while a snapshot is showing, or
    /// `· refresh failed` once its refresh has failed; `None` once a fetch
    /// has replaced it.
    pub fn cache_indicator(&self, now: DateTime<Utc>) -> Option<String> {
        let at = self.cached?;
        let what = if self.loading {
            "refreshing…"
        } else if self.cache_refresh_failed {
            "refresh failed"
        } else {
            "Ctrl+R refreshes"
        };
        Some(format!("cached {} · {what}", age(now - at)))
    }
}

/// A coarse age: `just now`, `12m ago`, `3h ago`, `2d ago`.
fn age(d: chrono::TimeDelta) -> String {
    let mins = d.num_minutes().max(0);
    match mins {
        0 => "just now".into(),
        1..60 => format!("{mins}m ago"),
        60..1440 => format!("{}h ago", mins / 60),
        _ => format!("{}d ago", mins / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::scope::FetchScope;
    use crate::tui::app::Loaded;

    fn app() -> App {
        App::new("org".into(), None, false, false, String::new())
    }

    fn snap(at: DateTime<Utc>) -> Snapshot {
        Snapshot::new(
            at,
            Loaded {
                scope: FetchScope::default(),
                repo: None,
            },
            vec![RepoIssues {
                repo: "r".into(),
                repo_url: "ru".into(),
                issues: vec![Issue::fixture("I_1", 1), Issue::fixture("I_2", 2)],
            }],
        )
    }

    #[test]
    fn an_applied_snapshot_is_the_list_until_a_fetch_lands() {
        let mut a = app();
        let at = Utc::now() - chrono::TimeDelta::hours(3);
        a.apply_snapshot(snap(at));
        assert_eq!(a.repos[0].issues.len(), 2);
        assert_eq!(a.loaded.as_ref().unwrap().repo, None);
        assert_eq!(a.cached, Some(at));
        assert_eq!(
            a.cache_indicator(Utc::now()).as_deref(),
            Some("cached 3h ago · refreshing…")
        );
        // The startup fetch lands.
        a.begin_fetch(a.wanted());
        a.finish_fetch(true);
        assert_eq!(a.cached, None);
        assert_eq!(a.cache_indicator(Utc::now()), None);
        assert!(a.fetched_at.unwrap() > at);
    }

    #[test]
    fn a_failed_refresh_keeps_the_snapshot_and_says_so() {
        let mut a = app();
        a.apply_snapshot(snap(Utc::now() - chrono::TimeDelta::minutes(5)));
        a.begin_fetch(a.wanted());
        a.loading = false;
        a.finish_fetch(false);
        assert!(a.cache_refresh_failed);
        assert_eq!(
            a.cache_indicator(Utc::now()).as_deref(),
            Some("cached 5m ago · refresh failed")
        );
    }

    #[test]
    fn a_widen_from_the_snapshot_keeps_what_is_loaded_and_adds_the_rest() {
        let mut a = app();
        // Loaded: open issues only, and one issue newer than the snapshot's.
        let newer = Issue {
            title: "edited since".into(),
            updated_at: Utc::now(),
            ..Issue::fixture("I_1", 1)
        };
        a.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "ru".into(),
            issues: vec![newer, Issue::fixture("I_new", 3)],
        }]);
        a.loaded = Some(Loaded {
            scope: FetchScope::default(),
            repo: Some("r".into()),
        });
        a.fetched_at = Some(Utc::now());
        a.loading = false;
        let old = Utc::now() - chrono::TimeDelta::hours(2);
        let mut s = snap(old);
        s.loaded.scope.include_closed = true;
        s.repos[0].issues[0].updated_at = old;
        s.repos.push(RepoIssues {
            repo: "q".into(),
            repo_url: "qu".into(),
            issues: vec![Issue::fixture("I_q", 4)],
        });
        a.widen_from_snapshot(s);

        let ids: Vec<_> = a
            .repos
            .iter()
            .flat_map(|r| r.issues.iter().map(|i| i.id.as_str()))
            .collect();
        for id in ["I_1", "I_2", "I_new", "I_q"] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        let one = a
            .repos
            .iter()
            .flat_map(|r| &r.issues)
            .find(|i| i.id == "I_1");
        assert_eq!(one.unwrap().title, "edited since", "the newer copy wins");
        assert!(a.loaded.as_ref().unwrap().scope.include_closed);
        assert_eq!(a.fetched_at, Some(old), "as old as its oldest part");
        assert_eq!(
            a.cache_indicator(Utc::now()).as_deref(),
            Some("cached 2h ago · Ctrl+R refreshes")
        );
    }

    #[test]
    fn the_snapshot_taken_is_what_is_loaded() {
        let mut a = app();
        assert!(a.snapshot().is_none(), "nothing loaded yet");
        let at = Utc::now();
        a.apply_snapshot(snap(at));
        let s = a.snapshot().unwrap();
        assert_eq!(s.saved_at, at);
        assert_eq!(s.repos[0].issues.len(), 2);
    }

    #[test]
    fn every_settle_bumps_the_data_generation() {
        let mut a = app();
        let g0 = a.data_gen;
        a.apply_snapshot(snap(Utc::now()));
        let g1 = a.data_gen;
        assert!(g1 > g0);
        assert!(a.patch_issue(Issue::fixture("I_1", 1)));
        let g2 = a.data_gen;
        assert!(g2 > g1);
        assert!(a.remove_issue("I_2"));
        let g3 = a.data_gen;
        assert!(g3 > g2);
        a.replace_repo("r", None);
        assert!(a.data_gen > g3);
    }

    #[test]
    fn ages_are_coarse() {
        let m = chrono::TimeDelta::minutes;
        assert_eq!(age(m(0)), "just now");
        assert_eq!(age(m(-3)), "just now");
        assert_eq!(age(m(12)), "12m ago");
        assert_eq!(age(m(60)), "1h ago");
        assert_eq!(age(m(60 * 49)), "2d ago");
    }
}
