//! What a fetch asks the backend for (#230).
//!
//! A [`FetchScope`] only ever *shrinks* a fetch: the client-side filters still
//! run on every loaded issue, so a scope may let through more than is shown
//! but must never leave out an issue the filters would show (ADR 0008).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Parameters pushed down to the backend. GitHub honours all of them via
/// `filterBy`; Linear and Jira use `include_closed` only and ignore the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The narrowest scope that covers both `self` and `other` (#226), so a
    /// refresh never fetches less than the list already holds. A login is
    /// kept only when both name the same one; labels are any-of, so the
    /// union of two label sets is both sets, and an empty set is no label
    /// pushdown at all.
    #[must_use]
    pub fn union(&self, other: &FetchScope) -> FetchScope {
        let login = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => Some(a.clone()),
            _ => None,
        };
        let labels = if self.labels.is_empty() || other.labels.is_empty() {
            Vec::new()
        } else {
            let mut labels = self.labels.clone();
            for l in &other.labels {
                if !labels.iter().any(|h| h.eq_ignore_ascii_case(l)) {
                    labels.push(l.clone());
                }
            }
            labels
        };
        FetchScope {
            include_closed: self.include_closed || other.include_closed,
            assignee: login(&self.assignee, &other.assignee),
            author: login(&self.author, &other.author),
            updated_since: match (self.updated_since, other.updated_since) {
                (Some(a), Some(b)) => Some(a.min(b)),
                _ => None,
            },
            labels,
        }
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

    /// Every pair of these, unioned, covers both inputs (#226).
    fn samples() -> Vec<FetchScope> {
        let login = |s: &str| Some(s.to_string());
        vec![
            FetchScope::default(),
            FetchScope {
                include_closed: true,
                ..FetchScope::default()
            },
            FetchScope {
                assignee: login("pgmac"),
                ..FetchScope::default()
            },
            FetchScope {
                assignee: login("PGMAC"),
                author: login("bot"),
                ..FetchScope::default()
            },
            FetchScope {
                author: login("other"),
                updated_since: at("2026-10-01T00:00:00Z"),
                ..FetchScope::default()
            },
            FetchScope {
                updated_since: at("2026-11-01T00:00:00Z"),
                labels: vec!["high".into(), "priority:high".into()],
                ..FetchScope::default()
            },
            FetchScope {
                labels: vec!["low".into()],
                include_closed: true,
                ..FetchScope::default()
            },
        ]
    }

    #[test]
    fn a_union_covers_both_of_its_inputs() {
        for a in samples() {
            for b in samples() {
                let u = a.union(&b);
                assert!(u.covers(&a), "{u:?} should cover {a:?}");
                assert!(u.covers(&b), "{u:?} should cover {b:?}");
            }
        }
    }

    #[test]
    fn a_union_with_itself_or_a_narrower_scope_changes_nothing() {
        let wide = FetchScope {
            include_closed: true,
            ..FetchScope::default()
        };
        let narrow = FetchScope {
            assignee: Some("pgmac".into()),
            ..FetchScope::default()
        };
        assert_eq!(wide.union(&wide), wide);
        assert_eq!(wide.union(&narrow), wide);
        assert_eq!(narrow.union(&narrow), narrow);
    }

    #[test]
    fn label_sets_merge_without_case_duplicates() {
        let a = FetchScope {
            labels: vec!["high".into()],
            ..FetchScope::default()
        };
        let b = FetchScope {
            labels: vec!["HIGH".into(), "low".into()],
            ..FetchScope::default()
        };
        assert_eq!(a.union(&b).labels, vec!["high".to_string(), "low".into()]);
    }
}
