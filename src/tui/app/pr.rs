use super::prelude::*;

/// PR-summary state: which links were found, which PR is being shown,
/// and the popup's scroll and selection.
impl App {
    pub(super) fn clear_pr_state(&mut self) {
        self.pr_links.clear();
        self.pr_target = None;
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.pr_sel = 0;
    }

    /// PR links referenced by the selected issue's body and its loaded
    /// comment thread, body first then comments in display order.
    pub fn collect_pr_links(&self) -> Vec<PrRef> {
        let mut text = String::new();
        if let Some(issue) = self.selected_issue() {
            text.push_str(&issue.body);
            text.push('\n');
        }
        if let Some(comments) = &self.detail_comments {
            for c in comments {
                text.push_str(&c.body);
                text.push('\n');
            }
        }
        parse_pr_links(&text)
    }

    /// Open the summary popup for a single PR; the caller spawns the fetch.
    pub fn open_pr_summary(&mut self, pr: PrRef) {
        self.pr_target = Some(pr);
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.pr_sel = 0;
        self.mode = Mode::PrSummary;
    }

    /// Open a picker over several candidate PR links.
    pub fn open_pr_picker(&mut self, links: Vec<PrRef>) {
        self.select_options = links.iter().map(PrRef::label).collect();
        self.select_idx = 0;
        self.select_filter.clear();
        self.pr_links = links;
        self.mode = Mode::PrPicker;
    }

    /// Deliver a PR summary fetch. Dropped if `pr` is no longer the target
    /// (the popup was closed or retargeted before the response landed).
    pub fn set_pr_summary(&mut self, pr: &PrRef, result: Result<PrSummary, String>) {
        if self.pr_target.as_ref() == Some(pr) {
            self.pr_summary = Some(result);
        }
    }

    /// Close the PR summary popup, back to the detail pane.
    pub fn close_pr_summary(&mut self) {
        self.pr_target = None;
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.pr_sel = 0;
        self.mode = Mode::Normal;
    }

    /// `r`: re-fetch the open PR summary in place. The caller re-spawns the
    /// fetch for `pr_target`; this just resets to the loading state.
    pub fn refresh_pr_summary(&mut self) {
        self.pr_summary = None;
        self.pr_scroll = 0;
        self.pr_sel = 0;
    }

    /// `Tab` (`delta = 1`) / `Shift+Tab` (`delta = -1`): cycle the PR summary
    /// selection among its open-able rows, wrapping at both ends, snapping
    /// the scroll to bring the newly selected row into view. No-op while
    /// the summary hasn't loaded.
    ///
    /// `targets` comes from `ui::pr_targets`, which reads them off the same
    /// row model the popup draws — this module deliberately does not compute
    /// them, so there is nothing here that could fall out of step with the
    /// rendered layout.
    pub fn select_pr_target(&mut self, delta: isize, targets: &[PrTarget]) {
        if targets.is_empty() {
            return;
        }
        let n = targets.len() as isize;
        let next = (self.pr_sel as isize + delta).rem_euclid(n) as usize;
        self.pr_sel = next;
        self.pr_scroll = targets[next].line;
    }

    /// URL the currently selected PR summary row would open with `o`/Enter.
    pub fn pr_selected_url(&self, targets: &[PrTarget]) -> Option<String> {
        targets.get(self.pr_sel).map(|t| t.url.clone())
    }

    /// Close the PR picker without selecting anything.
    pub fn close_pr_picker(&mut self) {
        self.pr_links.clear();
        self.mode = Mode::Normal;
    }
}
