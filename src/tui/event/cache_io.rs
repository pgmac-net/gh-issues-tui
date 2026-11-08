//! The event loop's side of the on-disk cache (#226, ADR 0010).
//!
//! `cache/` owns the files; `app/cache.rs` turns a snapshot into list data.
//! This decides when: draw a usable snapshot at startup, write a couple of
//! seconds after the data last settled and on quit, and swap everything when
//! the org changes. Every write is best-effort, as with the layout (#236).

use std::path::PathBuf;
use std::time::Duration;

use tokio::time::Instant;

use crate::cache::answers::{self, ReadinessCache, SearchCache};
use crate::cache::run_jobs::{self, RunJobsCache};
use crate::cache::{self, snapshot};
use crate::tui::app::App;

/// How long after the last settle the snapshot is written.
const DEBOUNCE: Duration = Duration::from_secs(2);

pub(crate) struct CacheIo {
    /// Config `cache`. `false`: never read, never write.
    enabled: bool,
    /// Jev answers may be cached: `enabled` and the issue-text consent.
    jev: bool,
    root: PathBuf,
    provider: String,
    host: String,
    /// The org whose files are in use; changes on `w`.
    org: String,
    /// The data generation last written (or read).
    written_gen: u64,
    due: Option<Instant>,
}

impl CacheIo {
    pub(crate) fn new(
        enabled: bool,
        jev: bool,
        root: PathBuf,
        provider: String,
        host: String,
        org: String,
    ) -> Self {
        Self {
            enabled,
            jev: enabled && jev,
            root,
            provider,
            host,
            org,
            written_gen: 0,
            due: None,
        }
    }

    fn dir(&self) -> PathBuf {
        cache::org_dir(&self.root, &self.provider, &self.host, &self.org)
    }

    /// Before the first fetch: `--no-cache` wipes this org's files; then a
    /// usable snapshot is drawn and the answer caches are loaded. Returns
    /// whether a snapshot was drawn, so the caller can rank its labels.
    pub(crate) fn start(&mut self, app: &mut App, no_cache: bool) -> bool {
        if no_cache {
            let _ = cache::wipe(&self.dir());
        }
        self.open(app)
    }

    /// Load the current org's snapshot (if usable) and answer caches.
    fn open(&mut self, app: &mut App) -> bool {
        let dir = self.dir();
        if self.jev {
            app.readiness
                .set_disk(ReadinessCache::load(answers::readiness_path(&dir)));
            app.search_disk = SearchCache::load(answers::search_path(&dir));
        } else {
            app.readiness.set_disk(ReadinessCache::disabled());
            app.search_disk = SearchCache::disabled();
        }
        // The completed-run cache (#266) holds no issue text, so only `cache`
        // gates it, not the Jev consent.
        app.repo_runs.disk = if self.enabled {
            RunJobsCache::load(run_jobs::path(&dir))
        } else {
            RunJobsCache::disabled()
        };
        if !self.enabled {
            return false;
        }
        let drawn = match snapshot::load(&snapshot::path(&dir)) {
            Some(s) if s.usable_for(&app.wanted()) => {
                app.apply_snapshot(s);
                true
            }
            _ => false,
        };
        // What was just read needs no writing back.
        self.written_gen = app.data_gen;
        drawn
    }

    /// A filter change reached past the loaded scope: if the snapshot on disk
    /// covers it, fold that in and fetch nothing (#226). Returns whether it
    /// did; otherwise the caller's widen fetch goes out as before.
    pub(crate) fn widen_from_snapshot(&mut self, app: &mut App) -> bool {
        if !self.enabled || !app.widened() {
            return false;
        }
        let Some(s) = snapshot::load(&snapshot::path(&self.dir())) else {
            return false;
        };
        if !s.usable_for(&app.wanted()) {
            return false;
        }
        app.widen_from_snapshot(s);
        app.status = Some("filters widened: shown from the cache".into());
        true
    }

    /// Note the data after an event. A settle arms the debounce.
    pub(crate) fn observe(&mut self, app: &App, now: Instant) {
        if self.enabled && app.data_gen != self.written_gen && self.due.is_none() {
            self.due = Some(now + DEBOUNCE);
        }
    }

    /// When the next write is due, for the event loop's timer.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The debounce fired, or the app is quitting: write what is loaded.
    pub(crate) fn flush(&mut self, app: &App) {
        self.due = None;
        if !self.enabled || app.org != self.org {
            return;
        }
        if let Some(s) = app.snapshot() {
            let _ = snapshot::save(&snapshot::path(&self.dir()), &s);
        }
        self.written_gen = app.data_gen;
    }

    /// The org changed (`w`). The old org's data is already gone from the
    /// app, so at most one debounce of its latest settles is not written;
    /// its last written snapshot stands. Then the new org's files open.
    pub(crate) fn sync_org(&mut self, app: &mut App) -> bool {
        if app.org == self.org {
            return false;
        }
        self.due = None;
        self.org = app.org.clone();
        self.open(app)
    }

    /// On quit: write if anything settled since the last write.
    pub(crate) fn finish(&mut self, app: &App) {
        if app.data_gen != self.written_gen {
            self.flush(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::cache::snapshot::Snapshot;
    use crate::provider::scope::FetchScope;
    use crate::provider::types::{Issue, RepoIssues};
    use crate::tui::app::Loaded;

    fn app(org: &str) -> App {
        App::new(org.into(), None, false, false, String::new())
    }

    fn io(root: &std::path::Path, org: &str, enabled: bool) -> CacheIo {
        CacheIo::new(
            enabled,
            true,
            root.to_path_buf(),
            "github".into(),
            "github.com".into(),
            org.into(),
        )
    }

    fn snap(include_closed: bool) -> Snapshot {
        Snapshot::new(
            Utc::now(),
            Loaded {
                scope: FetchScope {
                    include_closed,
                    ..FetchScope::default()
                },
                repo: None,
            },
            vec![RepoIssues {
                repo: "r".into(),
                repo_url: "ru".into(),
                issues: vec![Issue::fixture("I_1", 1)],
            }],
        )
    }

    fn put(root: &std::path::Path, org: &str, s: &Snapshot) {
        let dir = cache::org_dir(root, "github", "github.com", org);
        snapshot::save(&snapshot::path(&dir), s).unwrap();
    }

    fn saved(root: &std::path::Path, org: &str) -> Option<Snapshot> {
        snapshot::load(&snapshot::path(&cache::org_dir(
            root,
            "github",
            "github.com",
            org,
        )))
    }

    #[test]
    fn a_usable_snapshot_is_drawn_at_startup() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "o", &snap(false));
        let mut a = app("o");
        assert!(io(tmp.path(), "o", true).start(&mut a, false));
        assert_eq!(a.repos[0].issues[0].id, "I_1");
        assert!(a.cached.is_some());
    }

    #[test]
    fn a_snapshot_that_does_not_cover_the_view_is_not_drawn() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "o", &snap(false));
        let mut a = app("o");
        a.state_filter = crate::tui::app::StateFilter::All;
        assert!(!io(tmp.path(), "o", true).start(&mut a, false));
        assert!(a.repos.is_empty());
    }

    #[test]
    fn cache_off_neither_draws_nor_writes() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "o", &snap(false));
        let mut a = app("o");
        let mut c = io(tmp.path(), "o", false);
        assert!(!c.start(&mut a, false));
        a.apply_snapshot(snap(true));
        c.observe(&a, Instant::now());
        assert_eq!(c.due(), None);
        c.finish(&a);
        assert!(!saved(tmp.path(), "o").unwrap().loaded.scope.include_closed);
    }

    #[test]
    fn no_cache_wipes_the_org_before_starting() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "o", &snap(false));
        put(tmp.path(), "other", &snap(false));
        let mut a = app("o");
        assert!(!io(tmp.path(), "o", true).start(&mut a, true));
        assert!(saved(tmp.path(), "o").is_none());
        assert!(saved(tmp.path(), "other").is_some(), "only this org");
    }

    #[test]
    fn a_settle_is_written_after_the_debounce() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = app("o");
        let mut c = io(tmp.path(), "o", true);
        c.start(&mut a, false);
        let t0 = Instant::now();
        c.observe(&a, t0);
        assert_eq!(c.due(), None, "nothing settled yet");
        a.apply_snapshot(snap(true));
        c.observe(&a, t0);
        assert_eq!(c.due(), Some(t0 + DEBOUNCE));
        // Later settles inside the window do not push it out.
        c.observe(&a, t0 + Duration::from_secs(1));
        assert_eq!(c.due(), Some(t0 + DEBOUNCE));
        c.flush(&a);
        assert!(saved(tmp.path(), "o").unwrap().loaded.scope.include_closed);
        c.observe(&a, t0 + Duration::from_secs(5));
        assert_eq!(c.due(), None, "written; nothing new");
    }

    #[test]
    fn a_widen_the_snapshot_covers_needs_no_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "o", &snap(true));
        let mut a = app("o");
        let mut c = io(tmp.path(), "o", true);
        // Loaded open issues only, then asked for all of them.
        a.set_data(snap(false).repos);
        a.loaded = Some(snap(false).loaded);
        a.loading = false;
        a.state_filter = crate::tui::app::StateFilter::All;
        assert!(a.widened());
        assert!(c.widen_from_snapshot(&mut a));
        assert!(!a.widened(), "nothing left to fetch");
        // A widen the snapshot does not cover is left to the fetch.
        a.filters.updated_after = None;
        a.loaded = Some(Loaded {
            scope: FetchScope::default(),
            repo: Some("r".into()),
        });
        a.state_filter = crate::tui::app::StateFilter::Open;
        let mut c_off = io(tmp.path(), "o", false);
        assert!(!c_off.widen_from_snapshot(&mut a), "cache off");
    }

    #[test]
    fn switching_org_opens_the_new_orgs_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "beta", &snap(false));
        let mut a = app("alpha");
        let mut c = io(tmp.path(), "alpha", true);
        c.start(&mut a, false);
        a.switch_org("beta".into());
        assert!(c.sync_org(&mut a));
        assert_eq!(a.repos[0].issues[0].id, "I_1");
        assert!(!c.sync_org(&mut a), "same org: nothing to do");
    }
}
