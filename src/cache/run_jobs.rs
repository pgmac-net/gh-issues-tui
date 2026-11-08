//! The completed-run cache (#266, ADR 0014): jobs and steps of finished runs,
//! kept on disk so unfolding an old run costs nothing on later sessions.
//!
//! A finished run never changes. A re-run is a new attempt, and the attempt is
//! part of the key, so an entry can never be served for the wrong attempt. Only
//! finished runs go in ([`RunJobsCache::put_many`] skips the rest), and the run
//! list itself is never cached.
//!
//! Best-effort like the other caches: a failed write is dropped, a missing,
//! corrupt or old-version file reads as empty, and a cache built with
//! [`RunJobsCache::disabled`] answers nothing and writes nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::fsio;
use crate::ci::{CiJob, CiRun};

const VERSION: u32 = 1;

/// How many finished runs the file holds before the oldest written goes.
pub const MAX_RUNS: usize = 2000;

pub fn path(org_dir: &Path) -> PathBuf {
    org_dir.join("run_jobs.json")
}

/// Which run and attempt an entry is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunKey {
    pub owner: String,
    pub repo: String,
    pub run_id: i64,
    pub attempt: u32,
}

impl RunKey {
    pub fn of(owner: &str, repo: &str, run: &CiRun) -> Self {
        Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            run_id: run.id,
            attempt: run.run_attempt,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    key: RunKey,
    jobs: Vec<CiJob>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunJobsFile {
    version: u32,
    /// Oldest written first, which is the eviction order.
    runs: Vec<Entry>,
}

impl Default for RunJobsFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            runs: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct RunJobsCache {
    /// `None`: disabled, never read or written.
    path: Option<PathBuf>,
    file: RunJobsFile,
    /// Position of each key in `file.runs`.
    index: HashMap<RunKey, usize>,
    cap: usize,
}

impl RunJobsCache {
    pub fn load(path: PathBuf) -> Self {
        let file = fsio::read::<RunJobsFile>(&path)
            .filter(|f| f.version == VERSION)
            .unwrap_or_default();
        let mut cache = Self {
            path: Some(path),
            file,
            index: HashMap::new(),
            cap: MAX_RUNS,
        };
        cache.trim();
        cache
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn with_cap(path: PathBuf, cap: usize) -> Self {
        let mut c = Self::load(path);
        c.cap = cap;
        c.trim();
        c
    }

    pub fn get(&self, key: &RunKey) -> Option<&[CiJob]> {
        self.path.as_ref()?;
        let i = *self.index.get(key)?;
        Some(&self.file.runs[i].jobs)
    }

    /// Keep the jobs of every finished run in `runs` that has any, then write
    /// once. Active runs and runs with no jobs read are skipped: neither is
    /// final.
    pub fn put_many<'a>(
        &mut self,
        owner: &str,
        repo: &str,
        runs: impl IntoIterator<Item = &'a CiRun>,
    ) {
        if self.path.is_none() {
            return;
        }
        let mut changed = false;
        for run in runs {
            if run.is_active() || run.jobs.is_empty() {
                continue;
            }
            let key = RunKey::of(owner, repo, run);
            if self.index.contains_key(&key) {
                continue;
            }
            self.file.runs.push(Entry {
                key,
                jobs: run.jobs.clone(),
            });
            changed = true;
        }
        if changed {
            self.trim();
            if let Some(path) = &self.path {
                let _ = fsio::write(path, &self.file);
            }
        }
    }

    /// Drop the oldest entries past the cap, and rebuild the index.
    fn trim(&mut self) {
        let over = self.file.runs.len().saturating_sub(self.cap);
        self.file.runs.drain(..over);
        self.index = self
            .file
            .runs
            .iter()
            .enumerate()
            .map(|(i, e)| (e.key.clone(), i))
            .collect();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.file.runs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::RunStatus;

    fn job(id: i64) -> CiJob {
        CiJob {
            id,
            name: format!("job{id}"),
            status: RunStatus::Success,
            started_at: None,
            completed_at: None,
            html_url: None,
            steps: vec![],
        }
    }

    fn run(id: i64, attempt: u32, status: RunStatus) -> CiRun {
        CiRun {
            id,
            run_attempt: attempt,
            status,
            jobs: vec![job(id * 10)],
            ..CiRun::default()
        }
    }

    #[test]
    fn finished_runs_read_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(dir.path());
        let mut c = RunJobsCache::load(p.clone());
        c.put_many(
            "o",
            "r",
            &[run(1, 1, RunStatus::Success), run(2, 1, RunStatus::Failure)],
        );

        let c = RunJobsCache::load(p);
        assert_eq!(c.len(), 2);
        let key = RunKey::of("o", "r", &run(2, 1, RunStatus::Failure));
        assert_eq!(c.get(&key).unwrap()[0].id, 20);
    }

    #[test]
    fn active_runs_and_runs_without_jobs_are_never_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = RunJobsCache::load(path(dir.path()));
        let mut bare = run(3, 1, RunStatus::Success);
        bare.jobs.clear();
        c.put_many("o", "r", &[run(1, 1, RunStatus::Running), bare]);
        assert_eq!(c.len(), 0);
        assert!(!path(dir.path()).exists(), "nothing new, nothing written");
    }

    #[test]
    fn a_rerun_is_a_new_attempt_and_misses() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = RunJobsCache::load(path(dir.path()));
        c.put_many("o", "r", &[run(1, 1, RunStatus::Failure)]);
        assert!(
            c.get(&RunKey::of("o", "r", &run(1, 2, RunStatus::Success)))
                .is_none()
        );
    }

    #[test]
    fn the_oldest_written_goes_first_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(dir.path());
        let mut c = RunJobsCache::with_cap(p.clone(), 2);
        c.put_many("o", "r", &[run(1, 1, RunStatus::Success)]);
        c.put_many(
            "o",
            "r",
            &[run(2, 1, RunStatus::Success), run(3, 1, RunStatus::Success)],
        );
        let key = |id| RunKey::of("o", "r", &run(id, 1, RunStatus::Success));
        assert!(c.get(&key(1)).is_none());
        assert!(c.get(&key(2)).is_some() && c.get(&key(3)).is_some());
        assert_eq!(RunJobsCache::with_cap(p, 2).len(), 2);
    }

    #[test]
    fn disabled_answers_nothing_and_writes_nothing() {
        let mut c = RunJobsCache::disabled();
        c.put_many("o", "r", &[run(1, 1, RunStatus::Success)]);
        assert_eq!(c.len(), 0);
        assert!(
            c.get(&RunKey::of("o", "r", &run(1, 1, RunStatus::Success)))
                .is_none()
        );
    }

    #[test]
    fn the_same_run_in_another_repo_is_another_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = RunJobsCache::load(path(dir.path()));
        c.put_many("o", "a", &[run(1, 1, RunStatus::Success)]);
        assert!(
            c.get(&RunKey::of("o", "b", &run(1, 1, RunStatus::Success)))
                .is_none()
        );
    }
}
