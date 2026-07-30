use super::prelude::*;

/// The PR summary popup's state: the links discovered on the issue, which PR
/// is being fetched or shown, and the popup's scroll and selection.
///
/// These are cleared in three different combinations depending on why. The
/// combinations are named methods here rather than restated at each call
/// site, because the differences between them are deliberate and easy to get
/// wrong — see [`PrState::close`].
#[derive(Debug, Default)]
pub struct PrState {
    /// Candidate PR links, populated when more than one is found
    /// (`Mode::PrPicker`).
    pub links: Vec<PrRef>,
    /// The PR currently being fetched or shown; guards against a stale
    /// response landing after the target moved on.
    pub target: Option<PrRef>,
    /// `None` while the summary fetch for `target` is in flight.
    pub summary: Option<Result<PrSummary, String>>,
    pub scroll: u16,
    /// Index into `ui::pr_targets` — the open-able row `o`/Enter will open
    /// and the popup highlights. `Tab`/`Shift+Tab` move it.
    pub sel: usize,
}

impl PrState {
    /// Show a single PR: set the target and reset the view to loading.
    pub fn open(&mut self, pr: PrRef) {
        self.target = Some(pr);
        self.reset_view();
    }

    /// Close the summary popup.
    ///
    /// Deliberately **keeps `links`**: closing the summary returns to the
    /// picker's candidates, and refetching them would be wasted work. Use
    /// `PrState::default()` when the links should go too — that is what
    /// opening or closing the detail pane does.
    pub fn close(&mut self) {
        self.target = None;
        self.reset_view();
    }

    /// `r`: re-fetch in place. Keeps `target` so the response can be matched
    /// against it; only the rendered result is discarded.
    pub fn refresh(&mut self) {
        self.reset_view();
    }

    /// Deliver a fetch. Dropped if `pr` is no longer the target (the popup
    /// was closed or retargeted before the response landed).
    pub fn set_summary(&mut self, pr: &PrRef, result: Result<PrSummary, String>) {
        if self.target.as_ref() == Some(pr) {
            self.summary = Some(result);
        }
    }

    /// `Tab` (`delta = 1`) / `Shift+Tab` (`delta = -1`): cycle the selection
    /// among the open-able rows, wrapping at both ends, snapping the scroll
    /// to bring the newly selected row into view. No-op while the summary
    /// hasn't loaded.
    ///
    /// `targets` comes from `ui::pr_targets`, which reads them off the same
    /// row model the popup draws — this module deliberately does not compute
    /// them, so there is nothing here that could fall out of step with the
    /// rendered layout.
    pub fn select(&mut self, delta: isize, targets: &[PrTarget]) {
        if targets.is_empty() {
            return;
        }
        let n = targets.len() as isize;
        let next = (self.sel as isize + delta).rem_euclid(n) as usize;
        self.sel = next;
        self.scroll = targets[next].line;
    }

    /// `j`/`k`: move one row, never past the last row that has content.
    ///
    /// `max` comes from `ui::pr_max_scroll`, measured off the row model the
    /// popup draws — this module computes no geometry, so the bound is passed
    /// in rather than derived here.
    pub fn scroll_by(&mut self, delta: i16, max: u16) {
        let moved = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as u16)
        };
        self.scroll = moved.min(max);
    }

    /// Pull the scroll back inside the content after [`Self::select`] snapped
    /// it to a target's row. A target row is always within the content, so
    /// this can never scroll the selection off screen.
    pub fn clamp_scroll(&mut self, max: u16) {
        self.scroll = self.scroll.min(max);
    }

    /// `Home`/`g`: jump to the first row. Leaves `sel` untouched — Home/End
    /// move the viewport, not which row `o`/Enter would open.
    pub fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    /// `End`/`G`: jump to the row past which only blank space remains.
    pub fn scroll_to_bottom(&mut self, max: u16) {
        self.scroll = max;
    }

    /// URL the currently selected row would open with `o`/Enter.
    pub fn selected_url(&self, targets: &[PrTarget]) -> Option<String> {
        targets.get(self.sel).map(|t| t.url.clone())
    }

    /// The rendered summary and where we are within it — everything except
    /// which PR it is about, and which links were found.
    fn reset_view(&mut self) {
        self.summary = None;
        self.scroll = 0;
        self.sel = 0;
    }
}

impl App {
    /// PR links referenced by the selected issue's body and its loaded
    /// comment thread, body first then comments in display order.
    pub fn collect_pr_links(&self) -> Vec<PrRef> {
        let mut text = String::new();
        if let Some(issue) = self.selected_issue() {
            text.push_str(&issue.body);
            text.push('\n');
        }
        if let Some(comments) = &self.detail.comments {
            for c in comments {
                text.push_str(&c.body);
                text.push('\n');
            }
        }
        parse_pr_links(&text)
    }

    /// Open the summary popup for a single PR; the caller spawns the fetch.
    pub fn open_pr_summary(&mut self, pr: PrRef) {
        self.pr.open(pr);
        self.mode = Mode::PrSummary;
    }

    /// Open a picker over several candidate PR links.
    pub fn open_pr_picker(&mut self, links: Vec<PrRef>) {
        self.picker
            .start(links.iter().map(PrRef::label).collect(), 0);
        self.pr.links = links;
        self.mode = Mode::PrPicker;
    }

    /// Close the PR summary popup, back to the detail pane.
    pub fn close_pr_summary(&mut self) {
        self.pr.close();
        self.mode = Mode::Normal;
    }

    /// Close the PR picker without selecting anything.
    pub fn close_pr_picker(&mut self) {
        self.pr.links.clear();
        self.mode = Mode::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(n: u64) -> PrRef {
        PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: n,
        }
    }

    fn loaded() -> PrState {
        let mut s = PrState {
            links: vec![pr(1), pr(2)],
            ..PrState::default()
        };
        s.open(pr(1));
        s.scroll = 12;
        s.sel = 3;
        s.summary = Some(Err("boom".into()));
        s
    }

    /// The distinction the grouping exists to protect: closing the summary
    /// keeps the discovered links so the picker does not have to refetch.
    #[test]
    fn close_keeps_the_discovered_links() {
        let mut s = loaded();
        s.close();
        assert_eq!(s.links.len(), 2, "links must survive closing the summary");
        assert!(s.target.is_none());
        assert!(s.summary.is_none());
        assert_eq!((s.scroll, s.sel), (0, 0));
    }

    #[test]
    fn refresh_keeps_the_target_so_the_response_still_matches() {
        let mut s = loaded();
        s.refresh();
        assert_eq!(s.target, Some(pr(1)), "target must survive a refresh");
        assert!(s.summary.is_none());
        assert_eq!((s.scroll, s.sel), (0, 0));
        // A response for the still-current target is accepted.
        s.set_summary(&pr(1), Err("again".into()));
        assert!(s.summary.is_some());
    }

    /// Regression for the unbounded `j` (#102): the popup used to scroll past
    /// the end of its content into blank space, with nothing but `u16::MAX` to
    /// stop it. Markdown tables expand rows, so this got easier to hit.
    #[test]
    fn scrolling_down_stops_at_the_last_row_with_content() {
        let mut s = loaded();
        s.scroll = 0;
        for _ in 0..1000 {
            s.scroll_by(1, 7);
        }
        assert_eq!(s.scroll, 7, "j must stop at the measured bound");
    }

    #[test]
    fn scrolling_up_stops_at_the_top() {
        let mut s = loaded();
        s.scroll = 1;
        s.scroll_by(-1, 7);
        s.scroll_by(-1, 7);
        assert_eq!(s.scroll, 0, "k must not underflow");
    }

    /// `PageDown` is `scroll_by(page, max)` — pinning that a page step past
    /// the end clamps to `max`, same as an unbounded `j`.
    #[test]
    fn paging_down_past_the_end_clamps_to_the_bound() {
        let mut s = loaded();
        s.scroll = 0;
        s.scroll_by(20, 7);
        assert_eq!(s.scroll, 7, "PageDown must stop at the measured bound");
    }

    #[test]
    fn home_jumps_to_the_top_without_moving_the_selection() {
        let mut s = loaded();
        s.scroll = 5;
        let sel_before = s.sel;
        s.scroll_to_top();
        assert_eq!(s.scroll, 0);
        assert_eq!(s.sel, sel_before, "Home must not touch Tab's selection");
    }

    #[test]
    fn end_jumps_to_the_measured_bound_without_moving_the_selection() {
        let mut s = loaded();
        let sel_before = s.sel;
        s.scroll_to_bottom(7);
        assert_eq!(s.scroll, 7);
        assert_eq!(s.sel, sel_before, "End must not touch Tab's selection");
    }

    #[test]
    fn end_stays_at_zero_when_the_content_already_fits() {
        let mut s = loaded();
        s.scroll_to_bottom(0);
        assert_eq!(s.scroll, 0, "content that fits cannot scroll");
    }

    /// `select` snaps the scroll to a target's row without knowing the
    /// viewport, so the key handler pulls it back inside the content.
    #[test]
    fn clamp_scroll_pulls_a_snapped_selection_back_inside_the_content() {
        let mut s = loaded();
        s.scroll = 40;
        s.clamp_scroll(7);
        assert_eq!(s.scroll, 7);
        s.clamp_scroll(9);
        assert_eq!(s.scroll, 7, "clamping never scrolls further down");
    }

    #[test]
    fn default_discards_everything_including_links() {
        let s = PrState::default();
        assert!(s.links.is_empty());
        assert!(s.target.is_none());
        assert!(s.summary.is_none());
    }

    #[test]
    fn open_resets_the_view_and_retargets() {
        let mut s = loaded();
        s.open(pr(9));
        assert_eq!(s.target, Some(pr(9)));
        assert!(s.summary.is_none());
        assert_eq!((s.scroll, s.sel), (0, 0));
    }

    #[test]
    fn a_stale_response_is_dropped() {
        let mut s = PrState::default();
        s.open(pr(1));
        s.set_summary(&pr(2), Err("stale".into()));
        assert!(
            s.summary.is_none(),
            "response for a former target is dropped"
        );
    }

    #[test]
    fn select_wraps_and_snaps_the_scroll() {
        let targets: Vec<PrTarget> = [(0u16), 12, 19]
            .into_iter()
            .map(|line| PrTarget {
                url: format!("u{line}"),
                line,
            })
            .collect();
        let mut s = PrState::default();

        s.select(1, &targets);
        assert_eq!((s.sel, s.scroll), (1, 12));
        s.select(-1, &targets);
        assert_eq!((s.sel, s.scroll), (0, 0));
        // Wrapping backwards from the first row lands on the last.
        s.select(-1, &targets);
        assert_eq!((s.sel, s.scroll), (2, 19));
        assert_eq!(s.selected_url(&targets), Some("u19".to_string()));
    }

    #[test]
    fn select_is_a_noop_without_targets() {
        let mut s = PrState::default();
        s.select(1, &[]);
        assert_eq!((s.sel, s.scroll), (0, 0));
    }
}
