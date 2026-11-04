//! The refresh indicator's state (#250): when loading began, and whether the
//! redraw timer is needed. Drawing is `ui::list`; the glyphs and colours are
//! `tui::spinner`.

use std::time::{Duration, Instant};

use super::{App, Mode};

impl App {
    /// Keep `loading_since` in step with `loading`. Called once per event-loop
    /// pass, so the ~10 sites that set `loading` need no knowledge of the clock.
    pub fn sync_loading(&mut self, now: Instant) {
        match (self.loading, self.loading_since) {
            (true, None) => self.loading_since = Some(now),
            (false, Some(_)) => self.loading_since = None,
            _ => {}
        }
    }

    /// Time since the current load began; `None` when idle.
    pub fn loading_elapsed(&self, now: Instant) -> Option<Duration> {
        if !self.loading {
            return None;
        }
        self.loading_since.map(|t| now.saturating_duration_since(t))
    }

    /// The indicator needs a redraw timer: data is loading and the frame is
    /// ours. An attached session draws full-frame, so nothing would show.
    pub fn indicator_animating(&self) -> bool {
        self.loading && self.mode != Mode::Harness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("o".into(), None, false, true, "{number}".into())
    }

    #[test]
    fn the_clock_follows_loading_edges() {
        let mut a = app();
        let t0 = Instant::now();
        a.loading = false;
        a.sync_loading(t0);
        assert_eq!(a.loading_since, None);
        assert_eq!(a.loading_elapsed(t0), None);

        a.loading = true;
        a.sync_loading(t0);
        assert_eq!(a.loading_since, Some(t0));
        // A later pass while still loading keeps the original start.
        a.sync_loading(t0 + Duration::from_secs(3));
        assert_eq!(a.loading_since, Some(t0));
        assert_eq!(
            a.loading_elapsed(t0 + Duration::from_secs(3)),
            Some(Duration::from_secs(3))
        );

        a.loading = false;
        a.sync_loading(t0 + Duration::from_secs(4));
        assert_eq!(a.loading_since, None);
    }

    #[test]
    fn a_new_load_restarts_the_clock() {
        let mut a = app();
        let t0 = Instant::now();
        a.sync_loading(t0);
        a.loading = false;
        a.sync_loading(t0 + Duration::from_secs(1));
        a.loading = true;
        a.sync_loading(t0 + Duration::from_secs(9));
        assert_eq!(a.loading_since, Some(t0 + Duration::from_secs(9)));
    }

    #[test]
    fn animation_is_off_when_idle_and_behind_a_session() {
        let mut a = app();
        a.loading = true;
        assert!(a.indicator_animating());
        a.mode = Mode::Harness;
        assert!(!a.indicator_animating());
        a.mode = Mode::Normal;
        a.loading = false;
        assert!(!a.indicator_animating());
    }
}
