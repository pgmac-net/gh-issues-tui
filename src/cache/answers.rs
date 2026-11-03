//! Jev answer caches (#226): readiness and search answers kept on disk under
//! the hash of the exact request that produced them.
//!
//! An answer is stale exactly when what would be sent changes: an edited
//! body, a new comment, another query, a new model or reworded question.
//! There is no TTL and no `updated_at` in the key, so a label change that is
//! not sent to Jev does not cost a re-evaluation (ADR 0010).
//!
//! Both caches are best-effort: a write that fails is dropped silently, and a
//! cache built with [`ReadinessCache::disabled`] / [`SearchCache::disabled`]
//! answers nothing and writes nothing (`cache = false`, or no consent).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::fsio;
use crate::typesafe::readiness::Readiness;

const VERSION: u32 = 1;

/// How many distinct queries the search cache keeps per org.
pub const MAX_QUERIES: usize = 20;

pub fn readiness_path(org_dir: &Path) -> PathBuf {
    org_dir.join("readiness.json")
}

pub fn search_path(org_dir: &Path) -> PathBuf {
    org_dir.join("search.json")
}

// ---- readiness ----

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Judged {
    hash: u64,
    readiness: Readiness,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReadinessFile {
    version: u32,
    /// One entry per issue id: a new answer replaces the old, so the file is
    /// bounded by the number of issues.
    issues: HashMap<String, Judged>,
}

impl Default for ReadinessFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            issues: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ReadinessCache {
    /// `None`: disabled, never read or written.
    path: Option<PathBuf>,
    file: ReadinessFile,
}

impl ReadinessCache {
    pub fn load(path: PathBuf) -> Self {
        let file = fsio::read::<ReadinessFile>(&path)
            .filter(|f| f.version == VERSION)
            .unwrap_or_default();
        Self {
            path: Some(path),
            file,
        }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    /// The judgement for `issue_id` if it was made from a request hashing to
    /// `hash`.
    pub fn get(&self, issue_id: &str, hash: u64) -> Option<Readiness> {
        self.path.as_ref()?;
        self.file
            .issues
            .get(issue_id)
            .filter(|j| j.hash == hash)
            .map(|j| j.readiness)
    }

    pub fn put(&mut self, issue_id: &str, hash: u64, readiness: Readiness) {
        let Some(path) = &self.path else { return };
        self.file
            .issues
            .insert(issue_id.to_string(), Judged { hash, readiness });
        let _ = fsio::write(path, &self.file);
    }
}

// ---- search ----

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Scored {
    hash: u64,
    score: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Query {
    query: String,
    issues: HashMap<String, Scored>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchFile {
    version: u32,
    /// Least recently used first.
    queries: Vec<Query>,
}

impl Default for SearchFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            queries: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct SearchCache {
    path: Option<PathBuf>,
    file: SearchFile,
}

/// Queries that differ only in case or spacing share one slot. The hash still
/// covers the query exactly as sent, so they do not share answers.
pub fn normalise(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl SearchCache {
    pub fn load(path: PathBuf) -> Self {
        let file = fsio::read::<SearchFile>(&path)
            .filter(|f| f.version == VERSION)
            .unwrap_or_default();
        Self {
            path: Some(path),
            file,
        }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    /// The score for `issue_id` under `query`, if it came from a request
    /// hashing to `hash`.
    pub fn get(&self, query: &str, issue_id: &str, hash: u64) -> Option<f64> {
        self.path.as_ref()?;
        let q = normalise(query);
        self.file
            .queries
            .iter()
            .find(|e| e.query == q)?
            .issues
            .get(issue_id)
            .filter(|s| s.hash == hash)
            .map(|s| s.score)
    }

    /// Record a search's answers (possibly none, when every candidate hit) and
    /// mark the query most recently used, evicting the oldest past
    /// [`MAX_QUERIES`].
    pub fn put(&mut self, query: &str, answers: impl IntoIterator<Item = (String, u64, f64)>) {
        let Some(path) = &self.path else { return };
        let q = normalise(query);
        let mut entry = match self.file.queries.iter().position(|e| e.query == q) {
            Some(i) => self.file.queries.remove(i),
            None => Query {
                query: q,
                issues: HashMap::new(),
            },
        };
        for (id, hash, score) in answers {
            entry.issues.insert(id, Scored { hash, score });
        }
        self.file.queries.push(entry);
        let excess = self.file.queries.len().saturating_sub(MAX_QUERIES);
        self.file.queries.drain(..excess);
        let _ = fsio::write(path, &self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Readiness {
        Readiness {
            specifics: 0.9,
            criteria: 0.8,
            actionable: 0.95,
            blocked: 0.1,
            duplicate: 0.05,
        }
    }

    #[test]
    fn a_readiness_answer_hits_only_on_the_same_request() {
        let tmp = tempfile::tempdir().unwrap();
        let p = readiness_path(tmp.path());
        let mut c = ReadinessCache::load(p.clone());
        c.put("I_1", 7, ready());
        let c = ReadinessCache::load(p);
        assert_eq!(c.get("I_1", 7), Some(ready()));
        assert_eq!(c.get("I_1", 8), None);
        assert_eq!(c.get("I_2", 7), None);
    }

    #[test]
    fn a_new_answer_replaces_the_issues_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = ReadinessCache::load(readiness_path(tmp.path()));
        c.put("I_1", 7, ready());
        c.put("I_1", 8, ready());
        assert_eq!(c.get("I_1", 7), None);
        assert!(c.get("I_1", 8).is_some());
        assert_eq!(c.file.issues.len(), 1);
    }

    #[test]
    fn a_disabled_cache_answers_and_writes_nothing() {
        let mut r = ReadinessCache::disabled();
        r.put("I_1", 7, ready());
        assert_eq!(r.get("I_1", 7), None);
        let mut s = SearchCache::disabled();
        s.put("q", [("I_1".to_string(), 1, 0.9)]);
        assert_eq!(s.get("q", "I_1", 1), None);
    }

    #[test]
    fn search_answers_hit_per_query_issue_and_request() {
        let tmp = tempfile::tempdir().unwrap();
        let p = search_path(tmp.path());
        let mut c = SearchCache::load(p.clone());
        c.put(
            "Tree View",
            [("I_1".to_string(), 1, 0.9), ("I_2".into(), 2, 0.1)],
        );
        let c = SearchCache::load(p);
        assert_eq!(c.get("  tree   view ", "I_1", 1), Some(0.9));
        assert_eq!(c.get("tree view", "I_2", 2), Some(0.1));
        assert_eq!(c.get("tree view", "I_1", 99), None);
        assert_eq!(c.get("other", "I_1", 1), None);
    }

    #[test]
    fn only_the_last_twenty_queries_are_kept_and_use_refreshes_a_query() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = SearchCache::load(search_path(tmp.path()));
        for i in 0..MAX_QUERIES {
            c.put(&format!("q{i}"), [("I".to_string(), 1, 0.5)]);
        }
        // Using q0 again, even with every answer a hit, keeps it.
        c.put("q0", []);
        c.put("new", [("I".to_string(), 1, 0.5)]);
        assert_eq!(c.file.queries.len(), MAX_QUERIES);
        assert!(c.get("q0", "I", 1).is_some());
        assert!(c.get("q1", "I", 1).is_none());
        assert!(c.get("new", "I", 1).is_some());
    }
}
