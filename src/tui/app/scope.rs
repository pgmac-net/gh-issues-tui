//! Loaded scope and auto-widen (#230).
//!
//! The filters decide what is shown; the *wanted* scope is the part of them
//! the backend can apply, so a fetch brings back less. The app remembers the
//! scope its data was fetched at (`loaded`). A filter change that stays
//! inside it narrows and needs no fetch; one that reaches past it widens and
//! refetches the filtered scope once. Glossary: `CONTEXT.md`.

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use super::prelude::*;
use super::refresh::FetchTarget;
use crate::provider::scope::FetchScope;

/// What a fetch asked for: the backend scope, and the one repo it was
/// narrowed to (`None` is the whole org).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loaded {
    pub scope: FetchScope,
    pub repo: Option<String>,
}

impl Loaded {
    /// True when data fetched for `self` can serve `wanted` without a fetch.
    pub fn covers(&self, wanted: &Loaded) -> bool {
        self.scope.covers(&wanted.scope)
            && match (&self.repo, &wanted.repo) {
                (None, _) => true,
                (Some(have), Some(want)) => have.eq_ignore_ascii_case(want),
                (Some(_), None) => false,
            }
    }
}

impl App {
    /// The part of the current filters a fetch can push down.
    ///
    /// Labels push down for one filter group only: GitHub ORs every label it
    /// is given, and priority AND status must both match. Each value goes in
    /// both spellings, `high` and `priority:high`, because the client filter
    /// accepts either.
    pub fn wanted_scope(&self) -> FetchScope {
        let login = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let labels = match (
            self.filters.priority.is_empty(),
            self.filters.status.is_empty(),
        ) {
            (false, true) => label_spellings("priority", &self.filters.priority),
            (true, false) => label_spellings("status", &self.filters.status),
            _ => Vec::new(),
        };
        FetchScope {
            include_closed: self.include_closed || self.state_filter != StateFilter::Open,
            assignee: login(&self.filters.assignee),
            author: login(&self.filters.author),
            updated_since: self.filters.updated_after.and_then(local_midnight),
            labels,
        }
    }

    /// The fetch the current filters want: [`Self::wanted_scope`] over the
    /// filtered target.
    pub fn wanted(&self) -> Loaded {
        Loaded {
            scope: self.wanted_scope(),
            repo: match self.filtered_target() {
                FetchTarget::Org => None,
                FetchTarget::Repo(repo) => Some(repo),
            },
        }
    }

    /// The scope partial refreshes (`r`) use: the loaded one, so a refreshed
    /// group is never fetched differently from the groups around it.
    pub fn loaded_scope(&self) -> FetchScope {
        self.loaded
            .as_ref()
            .map_or_else(|| self.wanted_scope(), |l| l.scope.clone())
    }

    /// The scope a filtered fetch or auto-refresh asks for (#226): what the
    /// filters want, widened to cover what is already loaded, so a refresh
    /// never fetches less than the list holds and toggling a filter back and
    /// forth fetches at most once.
    pub fn refresh_scope(&self) -> FetchScope {
        let wanted = self.wanted_scope();
        match &self.loaded {
            Some(l) => wanted.union(&l.scope),
            None => wanted,
        }
    }

    /// True when the filters now reach past the loaded data and nothing
    /// stands in the way of refetching: data has loaded once, no fetch is
    /// out, no rate limit, and no popup that a data swap would disturb.
    pub fn widened(&self) -> bool {
        !self.loading
            && self.rate_limit_error.is_none()
            && matches!(self.mode, Mode::Normal | Mode::FilterMenu)
            && self
                .loaded
                .as_ref()
                .is_some_and(|loaded| !loaded.covers(&self.wanted()))
    }

    /// A fetch for `target` is going out; it becomes the loaded scope when
    /// it lands (see [`Self::finish_fetch`]).
    pub fn begin_fetch(&mut self, target: Loaded) {
        self.pending_fetch = Some(target);
    }

    /// The fetch begun last has landed. On success its target becomes the
    /// loaded scope; on failure the loaded scope stays what it was. A
    /// partial refresh never called `begin_fetch`, so it changes nothing.
    pub fn finish_fetch(&mut self, ok: bool) {
        if let Some(target) = self.pending_fetch.take()
            && ok
        {
            self.loaded = Some(target);
            // A fetch of the filtered scope or the org replaced the snapshot.
            self.fetched_at = Some(Utc::now());
            self.cached = None;
        }
    }
}

/// `value` and `<prefix>:value` for each filter value, as GitHub label names.
fn label_spellings(prefix: &str, values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        for name in [v.clone(), format!("{prefix}:{v}")] {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
                out.push(name);
            }
        }
    }
    out
}

/// The instant a local day starts. The client filter compares local days, so
/// pushing down local midnight lets through exactly the issues it keeps. A
/// day that starts inside a DST gap pushes nothing down (a superset).
fn local_midnight(day: NaiveDate) -> Option<DateTime<Utc>> {
    let midnight = day.and_hms_opt(0, 0, 0)?;
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::HelpTopic;

    fn app() -> App {
        App::new("org".into(), None, false, false, String::new())
    }

    #[test]
    fn unfiltered_wants_open_issues_only() {
        assert_eq!(app().wanted_scope(), FetchScope::default());
    }

    #[test]
    fn logins_and_since_push_down() {
        let mut a = app();
        a.filters.assignee = " pgmac ".into();
        a.filters.author = "someone".into();
        a.filters.updated_after = NaiveDate::from_ymd_opt(2026, 10, 1);
        let s = a.wanted_scope();
        assert_eq!(s.assignee.as_deref(), Some("pgmac"));
        assert_eq!(s.author.as_deref(), Some("someone"));
        assert_eq!(
            s.updated_since
                .map(|t| crate::localtime::local_date_in(t, &Local)),
            NaiveDate::from_ymd_opt(2026, 10, 1)
        );
    }

    #[test]
    fn one_label_group_pushes_down_in_both_spellings() {
        let mut a = app();
        a.filters.priority = vec!["high".into()];
        assert_eq!(a.wanted_scope().labels, vec!["high", "priority:high"]);
    }

    #[test]
    fn two_label_groups_push_neither() {
        let mut a = app();
        a.filters.priority = vec!["high".into()];
        a.filters.status = vec!["blocked".into()];
        assert!(a.wanted_scope().labels.is_empty());
    }

    #[test]
    fn showing_closed_wants_closed() {
        let mut a = app();
        a.state_filter = StateFilter::Closed;
        assert!(a.wanted_scope().include_closed);
    }

    #[test]
    fn a_refresh_never_fetches_less_than_is_loaded() {
        let mut a = app();
        assert_eq!(a.refresh_scope(), a.wanted_scope(), "nothing loaded yet");
        // Loaded with closed issues, then narrowed back to open ones.
        a.loaded = Some(Loaded {
            scope: FetchScope {
                include_closed: true,
                ..FetchScope::default()
            },
            repo: None,
        });
        a.filters.assignee = "pgmac".into();
        let s = a.refresh_scope();
        assert!(s.include_closed, "narrowing never shrinks the loaded scope");
        assert_eq!(s.assignee, None);
        // ...so toggling closed on again needs no fetch.
        a.state_filter = StateFilter::All;
        a.loading = false;
        a.begin_fetch(Loaded {
            scope: s,
            repo: None,
        });
        a.finish_fetch(true);
        assert!(!a.widened());
    }

    #[test]
    fn nothing_widens_before_the_first_load() {
        let mut a = app();
        a.loading = false;
        a.filters.assignee = "x".into();
        assert!(!a.widened());
    }

    #[test]
    fn narrowing_stays_loaded_and_widening_does_not() {
        let mut a = app();
        a.loading = false;
        a.begin_fetch(a.wanted());
        a.finish_fetch(true);
        a.filters.assignee = "pgmac".into();
        assert!(!a.widened(), "narrowing is served from loaded data");

        a.begin_fetch(a.wanted());
        a.finish_fetch(true);
        a.filters.assignee.clear();
        assert!(a.widened(), "clearing the assignee reaches past the load");
    }

    #[test]
    fn no_widen_while_loading_rate_limited_or_in_a_popup() {
        let mut a = app();
        a.loading = false;
        a.loaded = Some(Loaded {
            scope: FetchScope {
                assignee: Some("x".into()),
                ..FetchScope::default()
            },
            repo: None,
        });
        assert!(a.widened());
        a.loading = true;
        assert!(!a.widened());
        a.loading = false;
        a.rate_limit_error = Some("limited".into());
        assert!(!a.widened());
        a.rate_limit_error = None;
        a.mode = Mode::Help(HelpTopic::Keys);
        assert!(!a.widened());
    }

    #[test]
    fn a_failed_fetch_keeps_the_old_loaded_scope() {
        let mut a = app();
        let before = Loaded {
            scope: FetchScope::default(),
            repo: None,
        };
        a.loaded = Some(before.clone());
        a.begin_fetch(Loaded {
            scope: FetchScope::default(),
            repo: Some("r".into()),
        });
        a.finish_fetch(false);
        assert_eq!(a.loaded, Some(before));
        assert!(a.pending_fetch.is_none());
    }

    #[test]
    fn a_repo_load_does_not_cover_the_org() {
        let repo = Loaded {
            scope: FetchScope::default(),
            repo: Some("r".into()),
        };
        let org = Loaded {
            scope: FetchScope::default(),
            repo: None,
        };
        assert!(org.covers(&repo));
        assert!(!repo.covers(&org));
        assert!(repo.covers(&Loaded {
            repo: Some("R".into()),
            ..repo.clone()
        }));
    }
}
