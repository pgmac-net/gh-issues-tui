//! What a fetch asks the backend for (#230).
//!
//! A [`FetchScope`] only ever *shrinks* a fetch: the client-side filters still
//! run on every loaded issue, so a scope may let through more than is shown
//! but must never leave out an issue the filters would show (ADR 0008).

use chrono::{DateTime, Utc};

/// Parameters pushed down to the backend. GitHub honours all of them via
/// `filterBy`; Linear and Jira use `include_closed` only and ignore the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchScope {
    pub include_closed: bool,
    /// Issues assigned to this login.
    pub assignee: Option<String>,
    /// Issues opened by this login (`createdBy`).
    pub author: Option<String>,
    /// Issues updated at or after this instant (`since`).
    pub updated_since: Option<DateTime<Utc>>,
    /// Issues carrying **any** of these labels. GitHub's label filter is
    /// any-of (checked live: 43 + 17 = 60), so this can hold one filter
    /// group's values, never two groups that must both match.
    pub labels: Vec<String>,
}

impl FetchScope {
    /// True when every issue `wanted` could return is also returned by `self`,
    /// so data fetched at `self` can serve a view that wants `wanted` without
    /// a fetch. Logins and labels compare case-insensitively, as GitHub does.
    pub fn covers(&self, wanted: &FetchScope) -> bool {
        let login = |have: &Option<String>, want: &Option<String>| match (have, want) {
            (None, _) => true,
            (Some(h), Some(w)) => h.eq_ignore_ascii_case(w),
            (Some(_), None) => false,
        };
        (self.include_closed || !wanted.include_closed)
            && login(&self.assignee, &wanted.assignee)
            && login(&self.author, &wanted.author)
            && match (self.updated_since, wanted.updated_since) {
                (None, _) => true,
                (Some(h), Some(w)) => w >= h,
                (Some(_), None) => false,
            }
            && (self.labels.is_empty()
                || (!wanted.labels.is_empty()
                    && wanted
                        .labels
                        .iter()
                        .all(|w| self.labels.iter().any(|h| h.eq_ignore_ascii_case(w)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Option<DateTime<Utc>> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn the_unfiltered_scope_covers_everything_but_closed() {
        let all = FetchScope::default();
        let narrow = FetchScope {
            assignee: Some("pgmac".into()),
            labels: vec!["bug".into()],
            updated_since: at("2026-10-01T00:00:00Z"),
            ..FetchScope::default()
        };
        assert!(all.covers(&narrow));
        assert!(!all.covers(&FetchScope {
            include_closed: true,
            ..FetchScope::default()
        }));
    }

    #[test]
    fn clearing_or_changing_a_login_widens() {
        let mine = FetchScope {
            assignee: Some("pgmac".into()),
            ..FetchScope::default()
        };
        assert!(mine.covers(&FetchScope {
            assignee: Some("PGMAC".into()),
            ..FetchScope::default()
        }));
        assert!(!mine.covers(&FetchScope::default()));
        assert!(!mine.covers(&FetchScope {
            assignee: Some("someone".into()),
            ..FetchScope::default()
        }));
    }

    #[test]
    fn an_earlier_since_widens_and_a_later_one_narrows() {
        let oct = FetchScope {
            updated_since: at("2026-10-01T00:00:00Z"),
            ..FetchScope::default()
        };
        let nov = FetchScope {
            updated_since: at("2026-11-01T00:00:00Z"),
            ..FetchScope::default()
        };
        assert!(oct.covers(&nov));
        assert!(!nov.covers(&oct));
        assert!(!oct.covers(&FetchScope::default()));
    }

    #[test]
    fn labels_cover_any_subset_but_not_a_new_value() {
        let two = FetchScope {
            labels: vec!["high".into(), "priority:high".into()],
            ..FetchScope::default()
        };
        let one = FetchScope {
            labels: vec!["HIGH".into()],
            ..FetchScope::default()
        };
        let other = FetchScope {
            labels: vec!["low".into()],
            ..FetchScope::default()
        };
        assert!(two.covers(&one));
        assert!(!one.covers(&two));
        assert!(!two.covers(&other));
        assert!(!two.covers(&FetchScope::default()));
    }
}
