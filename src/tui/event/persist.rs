//! Saving and resuming the layout from the event loop (#236).
//!
//! `app/resume.rs` takes and applies snapshots; this owns the I/O around them:
//! load at startup, write a couple of seconds after the layout last changed
//! (so closing the terminal or a crash loses at most that much), write on
//! quit, and swap layouts when the org changes. Every write is best-effort: a
//! failure to save is never worth interrupting the app for.

use std::path::PathBuf;
use std::time::Duration;

use tokio::time::Instant;

use crate::tui::app::App;
use crate::tui::layout_state::{self, Layout};

/// How long after the last layout change the file is written.
const DEBOUNCE: Duration = Duration::from_secs(2);

pub(crate) struct LayoutSaver {
    /// `false` (config `restore_layout = false`) means never load, never save.
    enabled: bool,
    dir: PathBuf,
    provider: String,
    /// The org and clone the app started in. The clone names a view only
    /// while the org is the one it belongs to.
    startup_org: String,
    clone: Option<String>,
    /// The org the layout being tracked belongs to; changes on `w`.
    org: String,
    /// The latest snapshot, and when it is due to be written (if it changed).
    seen: Option<Layout>,
    due: Option<Instant>,
}

impl LayoutSaver {
    pub(crate) fn new(
        enabled: bool,
        dir: PathBuf,
        provider: String,
        org: String,
        clone: Option<String>,
    ) -> Self {
        Self {
            enabled,
            dir,
            provider,
            startup_org: org.clone(),
            clone,
            org,
            seen: None,
            due: None,
        }
    }

    /// The file for the org being tracked.
    fn path(&self) -> PathBuf {
        let clone = (self.org == self.startup_org)
            .then_some(self.clone.as_deref())
            .flatten();
        layout_state::path_in(&self.dir, &self.org, clone)
    }

    fn write(&self, layout: &Layout) {
        let _ = layout_state::save(&self.path(), layout);
    }

    /// Resume from the saved layout, before the first fetch. `fresh` skips
    /// loading (the session starts clean) but not the saving that follows.
    pub(crate) fn resume(&self, app: &mut App, fresh: bool) {
        if !self.enabled || fresh {
            return;
        }
        if let Some(layout) = layout_state::load(&self.path(), &self.provider) {
            // A repo detected from the clone is a startup input and wins.
            let keep_repo = self.clone.is_some() && app.org == self.startup_org;
            app.apply_layout(&layout, keep_repo);
        }
    }

    /// Note the app's layout after an event. A change arms the debounce.
    pub(crate) fn observe(&mut self, app: &App, now: Instant) {
        if !self.enabled {
            return;
        }
        let layout = app.layout(&self.provider);
        if self.seen.as_ref() != Some(&layout) {
            self.seen = Some(layout);
            self.due = Some(now + DEBOUNCE);
        }
    }

    /// When the next write is due, for the event loop's timer.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The debounce fired: write the latest snapshot.
    pub(crate) fn flush(&mut self) {
        self.due = None;
        if self.enabled
            && let Some(layout) = &self.seen
        {
            self.write(layout);
        }
    }

    /// The org changed (`w`): write the layout the old org was left in, then
    /// resume the new org's. The snapshot kept from before the switch is what
    /// the old org looked like; the app itself has already been reset.
    pub(crate) fn sync_org(&mut self, app: &mut App) {
        if !self.enabled || app.org == self.org {
            return;
        }
        if let Some(old) = self.seen.take() {
            self.write(&old);
        }
        self.due = None;
        self.org = app.org.clone();
        if let Some(layout) = layout_state::load(&self.path(), &self.provider) {
            app.apply_layout(&layout, false);
        }
    }

    /// On quit: write the layout as it stands now.
    pub(crate) fn finish(&mut self, app: &App) {
        if self.enabled {
            self.write(&app.layout(&self.provider));
        }
    }
}

/// The default directory, for the event loop.
pub(crate) fn default_dir() -> PathBuf {
    layout_state::dir()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::provider::types::{Issue, RepoIssues};
    use crate::tui::app::StateFilter;

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("gh-issues-persist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn app(org: &str, repo: Option<&str>) -> App {
        App::new(
            org.into(),
            repo.map(Into::into),
            false,
            false,
            String::new(),
        )
    }

    fn saver(dir: &Path, org: &str, clone: Option<&str>, enabled: bool) -> LayoutSaver {
        LayoutSaver::new(
            enabled,
            dir.to_path_buf(),
            "github".into(),
            org.into(),
            clone.map(Into::into),
        )
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn a_change_arms_the_debounce_and_the_flush_writes_it() {
        let dir = tmp("debounce");
        let mut s = saver(&dir, "o", None, true);
        let mut a = app("o", None);
        let t0 = Instant::now();

        s.observe(&a, t0);
        assert_eq!(s.due(), Some(t0 + DEBOUNCE), "the first look is a change");
        s.flush();
        assert_eq!(files(&dir), vec!["o.json"]);
        assert_eq!(s.due(), None);

        s.observe(&a, t0 + Duration::from_secs(5));
        assert_eq!(s.due(), None, "nothing changed, so nothing to write");

        a.state_filter = StateFilter::All;
        s.observe(&a, t0 + Duration::from_secs(6));
        assert_eq!(s.due(), Some(t0 + Duration::from_secs(6) + DEBOUNCE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_later_change_pushes_the_write_back() {
        let dir = tmp("pushback");
        let mut s = saver(&dir, "o", None, true);
        let mut a = app("o", None);
        let t0 = Instant::now();
        s.observe(&a, t0);
        a.sort_desc = !a.sort_desc;
        s.observe(&a, t0 + Duration::from_secs(1));
        assert_eq!(s.due(), Some(t0 + Duration::from_secs(1) + DEBOUNCE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quitting_writes_the_layout_as_it_stands() {
        let dir = tmp("quit");
        let mut s = saver(&dir, "o", None, true);
        let mut a = app("o", None);
        a.state_filter = StateFilter::Closed;
        s.finish(&a);
        let saved = layout_state::load(&layout_state::path_in(&dir, "o", None), "github").unwrap();
        assert_eq!(saved.state_filter, "closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_next_start_resumes_the_saved_filters() {
        let dir = tmp("resume");
        let mut first = app("o", None);
        first.filters.assignee = "pgmac".into();
        first.sort_desc = false;
        saver(&dir, "o", None, true).finish(&first);

        let mut second = app("o", None);
        saver(&dir, "o", None, true).resume(&mut second, false);
        assert_eq!(second.filters.assignee, "pgmac");
        assert!(!second.sort_desc);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_neither_loads_nor_saves() {
        let dir = tmp("disabled");
        let mut first = app("o", None);
        first.filters.assignee = "pgmac".into();
        saver(&dir, "o", None, true).finish(&first);

        let mut off = saver(&dir, "o", None, false);
        let mut a = app("o", None);
        off.resume(&mut a, false);
        assert_eq!(a.filters.assignee, "", "not loaded");
        a.filters.author = "x".into();
        off.observe(&a, Instant::now());
        assert_eq!(off.due(), None);
        off.finish(&a);
        let saved = layout_state::load(&layout_state::path_in(&dir, "o", None), "github").unwrap();
        assert_eq!(saved.filters.author, "", "not overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_starts_clean_but_still_saves() {
        let dir = tmp("fresh");
        let mut first = app("o", None);
        first.filters.assignee = "pgmac".into();
        saver(&dir, "o", None, true).finish(&first);

        let mut s = saver(&dir, "o", None, true);
        let mut a = app("o", None);
        s.resume(&mut a, true);
        assert_eq!(a.filters.assignee, "", "--fresh does not load");
        a.filters.author = "ann".into();
        s.finish(&a);
        let saved = layout_state::load(&layout_state::path_in(&dir, "o", None), "github").unwrap();
        assert_eq!(saved.filters.author, "ann", "but it saves on the way out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clone_and_the_whole_org_keep_separate_layouts() {
        let dir = tmp("views");
        let mut whole = app("o", None);
        whole.filters.assignee = "everyone".into();
        saver(&dir, "o", None, true).finish(&whole);
        let mut inside = app("o", Some("r"));
        inside.filters.assignee = "just-r".into();
        saver(&dir, "o", Some("r"), true).finish(&inside);
        assert_eq!(files(&dir), vec!["o.json", "o@r.json"]);

        let mut back = app("o", Some("r"));
        saver(&dir, "o", Some("r"), true).resume(&mut back, false);
        assert_eq!(back.filters.assignee, "just-r");
        assert_eq!(back.filters.repo, "r", "the clone's repo still wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_org_saves_the_old_view_and_resumes_the_new_one() {
        let dir = tmp("switch");
        let mut other = app("beta", None);
        other.filters.author = "from-beta".into();
        saver(&dir, "beta", None, true).finish(&other);

        let mut s = saver(&dir, "alpha", None, true);
        let mut a = app("alpha", None);
        a.filters.assignee = "ann".into();
        s.observe(&a, Instant::now());

        // `w` resets the app to a clean slate for the new org.
        a.switch_org("beta".into());
        s.sync_org(&mut a);
        assert_eq!(a.filters.author, "from-beta", "beta resumed");
        let alpha = layout_state::load(&layout_state::path_in(&dir, "alpha", None), "github");
        assert_eq!(
            alpha.unwrap().filters.assignee,
            "ann",
            "alpha was kept as left"
        );

        // Switching back finds alpha where it was.
        s.observe(&a, Instant::now());
        a.switch_org("alpha".into());
        s.sync_org(&mut a);
        assert_eq!(a.filters.assignee, "ann");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_selection_waiting_for_data_is_not_lost_by_an_early_save() {
        let dir = tmp("early");
        let mut ready = app("o", None);
        ready.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: String::new(),
            issues: vec![Issue::fixture("I_7", 7)],
        }]);
        ready.selected = 1;
        saver(&dir, "o", None, true).finish(&ready);

        // A second start quits before its data lands: the selection must survive.
        let mut s = saver(&dir, "o", None, true);
        let mut again = app("o", None);
        s.resume(&mut again, false);
        s.observe(&again, Instant::now());
        s.flush();
        s.finish(&again);
        let saved = layout_state::load(&layout_state::path_in(&dir, "o", None), "github").unwrap();
        assert_eq!(
            saved.selected,
            Some(layout_state::Selected::Issue("I_7".into()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
