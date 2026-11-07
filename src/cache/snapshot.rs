//! The snapshot (#226): the last loaded issues of one org, with the scope they
//! were fetched at, drawn at startup before the first fetch lands.
//!
//! Glossary: `CONTEXT.md` → Caching. Decision: ADR 0010.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::fsio;
use crate::provider::types::{RepoIssues, SubIssues};
use crate::tui::app::Loaded;

/// Bumped whenever the stored shape changes; an older file reads as missing.
const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    /// When the data was last settled from the backend.
    pub saved_at: DateTime<Utc>,
    /// What the data was fetched at.
    pub loaded: Loaded,
    pub repos: Vec<RepoIssues>,
    /// Outside sub-issues (#261, ADR 0013), drawn at startup with the rest.
    /// Not part of the loaded scope, so [`Self::usable_for`] ignores them.
    /// Missing in older files, which read as none fetched.
    #[serde(default)]
    pub outside: Vec<SubIssues>,
}

impl Snapshot {
    pub fn new(saved_at: DateTime<Utc>, loaded: Loaded, repos: Vec<RepoIssues>) -> Self {
        Self {
            version: VERSION,
            saved_at,
            loaded,
            repos,
            outside: Vec::new(),
        }
    }

    /// The same snapshot holding `outside` sub-issues too.
    pub fn with_outside(mut self, outside: Vec<SubIssues>) -> Self {
        self.outside = outside;
        self
    }

    /// A *usable* snapshot covers what is wanted. Any other is treated as
    /// missing, so it never stands in for data it does not hold (ADR 0010).
    pub fn usable_for(&self, wanted: &Loaded) -> bool {
        self.loaded.covers(wanted)
    }
}

/// The snapshot file in an org's cache directory.
pub fn path(org_dir: &Path) -> PathBuf {
    org_dir.join("snapshot.json")
}

/// The saved snapshot, if any. Missing, corrupt or another version is `None`.
pub fn load(path: &Path) -> Option<Snapshot> {
    fsio::read::<Snapshot>(path).filter(|s| s.version == VERSION)
}

pub fn save(path: &Path, snapshot: &Snapshot) -> std::io::Result<()> {
    fsio::write(path, snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::scope::FetchScope;
    use crate::provider::types::Issue;

    fn loaded(include_closed: bool, repo: Option<&str>) -> Loaded {
        Loaded {
            scope: FetchScope {
                include_closed,
                ..FetchScope::default()
            },
            repo: repo.map(str::to_string),
        }
    }

    fn repos() -> Vec<RepoIssues> {
        vec![RepoIssues {
            repo: "r".into(),
            repo_url: "ru".into(),
            issues: vec![Issue::fixture("I_1", 1)],
        }]
    }

    #[test]
    fn a_saved_snapshot_loads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path(tmp.path());
        let at = Utc::now();
        save(&p, &Snapshot::new(at, loaded(true, None), repos())).unwrap();
        let s = load(&p).unwrap();
        assert_eq!(s.saved_at, at);
        assert_eq!(s.loaded, loaded(true, None));
        assert_eq!(s.repos[0].issues[0].id, "I_1");
    }

    /// #261: outside sub-issues round-trip, and a file from before them
    /// (no `outside` key) still loads, with none.
    #[test]
    fn outside_sub_issues_round_trip_and_older_files_read_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path(tmp.path());
        let outside = vec![SubIssues {
            parent: "I_1".into(),
            children: vec![crate::provider::types::OutsideIssue {
                issue: Issue::fixture("I_9", 9),
                repo: "other".into(),
                owner: "o".into(),
            }],
        }];
        let s = Snapshot::new(Utc::now(), loaded(false, None), repos()).with_outside(outside);
        save(&p, &s).unwrap();
        let back = load(&p).unwrap();
        assert_eq!(back.outside[0].children[0].issue.id, "I_9");
        assert_eq!(back.outside[0].children[0].owner, "o");

        let mut old = serde_json::to_value(&s).unwrap();
        old.as_object_mut().unwrap().remove("outside");
        std::fs::write(&p, old.to_string()).unwrap();
        let back = load(&p).unwrap();
        assert!(back.outside.is_empty());
        assert!(
            back.usable_for(&loaded(false, None)),
            "outside never decides usability"
        );
    }

    #[test]
    fn another_version_reads_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path(tmp.path());
        let mut s = Snapshot::new(Utc::now(), loaded(false, None), repos());
        s.version = VERSION + 1;
        fsio::write(&p, &s).unwrap();
        assert!(load(&p).is_none());
    }

    #[test]
    fn usable_only_when_it_covers_what_is_wanted() {
        let s = Snapshot::new(Utc::now(), loaded(false, None), repos());
        assert!(s.usable_for(&loaded(false, None)));
        assert!(s.usable_for(&loaded(false, Some("r"))));
        // Open issues only cannot serve a view that shows closed ones.
        assert!(!s.usable_for(&loaded(true, None)));
        // One repo cannot serve the org.
        let one = Snapshot::new(Utc::now(), loaded(true, Some("r")), repos());
        assert!(!one.usable_for(&loaded(false, None)));
        assert!(one.usable_for(&loaded(false, Some("R"))));
    }
}
