use super::prelude::*;
use crate::tui::layout;

/// The detail pane's own state: whether the split is open, which region has
/// the keyboard selection, the loaded comment thread, and the two regions'
/// independent scroll offsets.
///
/// These are set up and torn down as a unit, so `Default` is what a closed
/// pane looks like and no caller has to remember the list.
#[derive(Debug, Default)]
pub struct DetailState {
    pub open: bool,
    pub sel: DetailSel,
    pub comments: Option<Vec<Comment>>,
    pub body_scroll: u16,
    pub comments_scroll: u16,
}

impl DetailState {
    /// Reset the selection and both scroll offsets to the top.
    pub fn reset_scroll(&mut self) {
        self.sel = DetailSel::Body;
        self.body_scroll = 0;
        self.comments_scroll = 0;
    }

    /// Number of loaded comments (0 while they are still fetching or absent).
    pub fn comment_count(&self) -> usize {
        self.comments.as_ref().map_or(0, Vec::len)
    }

    /// Tab (`delta = 1`) / Shift+Tab (`delta = -1`): cycle the selection
    /// through `Body → Comment(0) → … → Comment(n-1)` and wrap. Selecting the
    /// body leaves `body_scroll` where it was (so returning keeps your place);
    /// the caller snaps `comments_scroll` to the newly selected comment's top.
    pub fn select(&mut self, delta: isize) {
        let n = self.comment_count() as isize;
        // Positions: 0 = Body, 1..=n = Comment(0..n-1). `n + 1` slots total.
        let cur = match self.sel {
            DetailSel::Body => 0,
            DetailSel::Comment(i) => i as isize + 1,
        };
        let next = (cur + delta).rem_euclid(n + 1);
        self.sel = if next == 0 {
            DetailSel::Body
        } else {
            DetailSel::Comment((next - 1) as usize)
        };
    }

    /// Keep the selection valid after a new comment thread lands: a selection
    /// past the end falls back to the last comment, and an empty thread falls
    /// back to the body.
    pub fn clamp_sel(&mut self) {
        if let DetailSel::Comment(i) = self.sel {
            let n = self.comment_count();
            self.sel = match n {
                0 => DetailSel::Body,
                _ if i >= n => DetailSel::Comment(n - 1),
                _ => DetailSel::Comment(i),
            };
        }
    }

    /// `↓` (`dir = 1`) / `↑` (`dir = -1`) in the detail pane (#217): move a
    /// row cursor over `Body → Comment(0) → … → Comment(n-1)` without wrapping.
    ///
    /// While the selected item still has rows beyond the viewport edge in
    /// `dir` — the body's end or a comment's bottom going down, its top going
    /// up — the step scrolls one row. Once that edge is visible, it selects
    /// the neighbour and scrolls only as far as needed to show the neighbour's
    /// nearest row. The selection is a whole item; there is no per-line cursor.
    pub fn step(&mut self, dir: isize, g: &DetailGeometry) {
        let n = g.comment_heights.len();
        let body_max = g.body_height.saturating_sub(g.body_view);
        match (self.sel, dir.signum()) {
            (DetailSel::Body, 1) => {
                if self.body_scroll < body_max {
                    self.body_scroll += 1;
                } else if n > 0 {
                    self.sel = DetailSel::Comment(0);
                    self.reveal_row(0, g);
                }
            }
            (DetailSel::Body, -1) => self.body_scroll = self.body_scroll.saturating_sub(1),
            (DetailSel::Comment(i), 1) => {
                let bottom = g.comment_top(i) + g.comment_heights.get(i).copied().unwrap_or(0);
                if g.comments_view > 0
                    && bottom > self.comments_scroll + g.comments_view
                    && self.comments_scroll < g.comments_max()
                {
                    self.comments_scroll += 1;
                } else if i + 1 < n {
                    self.sel = DetailSel::Comment(i + 1);
                    self.reveal_row(g.comment_top(i + 1), g);
                }
            }
            (DetailSel::Comment(i), -1) => {
                if g.comments_view > 0 && g.comment_top(i) < self.comments_scroll {
                    self.comments_scroll -= 1;
                } else if i == 0 {
                    self.sel = DetailSel::Body;
                } else {
                    self.sel = DetailSel::Comment(i - 1);
                    self.reveal_row(g.comment_top(i).saturating_sub(1), g);
                }
            }
            _ => {}
        }
    }

    /// `PgDn` / `PgUp`: [`Self::step`] repeated one viewport's worth of times —
    /// the viewport of the region the selection starts in.
    pub fn page(&mut self, dir: isize, g: &DetailGeometry) {
        let rows = match self.sel {
            DetailSel::Body => g.body_view,
            DetailSel::Comment(_) => g.comments_view,
        };
        for _ in 0..rows.max(1) {
            self.step(dir, g);
        }
    }

    /// `g` / `Home`: select the body and scroll both regions to the top.
    pub fn jump_top(&mut self) {
        self.reset_scroll();
    }

    /// `G` / `End`: select the last comment and scroll the thread to its end.
    /// With no comments the body is the last item, so its end is shown.
    pub fn jump_end(&mut self, g: &DetailGeometry) {
        match g.comment_heights.len() {
            0 => {
                self.sel = DetailSel::Body;
                self.body_scroll = g.body_height.saturating_sub(g.body_view);
            }
            n => {
                self.sel = DetailSel::Comment(n - 1);
                self.comments_scroll = g.comments_max();
            }
        }
    }

    /// `Tab` / `Shift+Tab` after [`Self::select`]: bring the newly selected
    /// comment into view by the minimal edge scroll — no snap to the top.
    pub fn reveal_selected(&mut self, g: &DetailGeometry) {
        if let DetailSel::Comment(i) = self.sel {
            let top = g.comment_top(i);
            let bottom = top + g.comment_heights.get(i).copied().unwrap_or(0);
            self.comments_scroll = layout::scroll_into_view(
                self.comments_scroll,
                top,
                bottom,
                g.comments_view,
                g.comments_total(),
            );
        }
    }

    /// The selected comment's header row on screen, relative to the comments
    /// viewport's top. Taken before a fold changes the thread's geometry and
    /// handed to [`Self::after_fold`]. `None` with the body selected.
    pub fn fold_anchor(&self, g: &DetailGeometry) -> Option<i32> {
        match self.sel {
            DetailSel::Comment(i) => Some(g.comment_top(i) as i32 - self.comments_scroll as i32),
            DetailSel::Body => None,
        }
    }

    /// After a fold or unfold (#210, #217): keep the selected comment's header
    /// on the screen row it occupied before (`anchor`, from
    /// [`Self::fold_anchor`]), then clamp to the thread's new extent — the
    /// only case the view moves is a thread that became shorter than the
    /// viewport. With the body selected the offset is only clamped.
    pub fn after_fold(&mut self, anchor: Option<i32>, g: &DetailGeometry) {
        if let (Some(row), DetailSel::Comment(i)) = (anchor, self.sel) {
            let top = g.comment_top(i);
            self.comments_scroll = (top as i32 - row).max(0) as u16;
            // A fold can shrink the selection out of a viewport that sat
            // below its header; bring it back rather than leave it off screen.
            let bottom = top + g.comment_heights.get(i).copied().unwrap_or(0);
            if bottom <= self.comments_scroll || top >= self.comments_scroll + g.comments_view {
                self.reveal_row(top, g);
            }
        }
        self.comments_scroll = self.comments_scroll.min(g.comments_max());
    }

    /// Scroll the comments region just far enough that `row` is visible.
    fn reveal_row(&mut self, row: u16, g: &DetailGeometry) {
        self.comments_scroll = layout::scroll_into_view(
            self.comments_scroll,
            row,
            row + 1,
            g.comments_view,
            g.comments_total(),
        );
    }
}

/// The detail pane's geometry at the live terminal size, measured by the key
/// handler with the renderer's own height functions (`ui::body_content_height`,
/// `ui::comment_height`) — `app/` does no wrapping of its own — and handed to
/// [`DetailState::step`] and friends (#217).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailGeometry {
    /// Wrapped height of the body region's content.
    pub body_height: u16,
    /// The body region's inner (viewport) height.
    pub body_view: u16,
    /// Each comment card's wrapped height, folded cards counting one row.
    pub comment_heights: Vec<u16>,
    /// The comments region's inner (viewport) height; 0 when it isn't drawn.
    pub comments_view: u16,
}

impl DetailGeometry {
    /// Row offset of comment `i`'s header within the stacked thread.
    pub fn comment_top(&self, i: usize) -> u16 {
        self.comment_heights
            .iter()
            .take(i)
            .fold(0u16, |acc, h| acc.saturating_add(*h))
    }

    /// Height of the whole stacked thread.
    pub fn comments_total(&self) -> u16 {
        self.comment_top(self.comment_heights.len())
    }

    /// Largest `comments_scroll` that still fills the viewport.
    pub fn comments_max(&self) -> u16 {
        self.comments_total().saturating_sub(self.comments_view)
    }
}

impl App {
    pub fn open_detail(&mut self) {
        self.detail.open = true;
        self.focus = Focus::Detail;
        self.detail.reset_scroll();
        self.detail.comments = None;
        self.pr = PrState::default();
    }

    /// Point the detail pane at issue `id`'s comment thread, returning the id
    /// only when a request is actually required.
    ///
    /// Two cases resolve without touching the API (#107): a thread already
    /// fetched this cycle, and an issue whose `comment_count` is zero — the
    /// bulk list fetch already told us there is nothing to load, so asking is
    /// pure waste. Both settle `detail.comments` to a loaded (possibly empty)
    /// thread rather than leaving it `None`, so the pane never shows a
    /// spinner for a thread that will never arrive.
    pub fn load_comments(&mut self, id: String) -> Option<String> {
        if let Some(cached) = self.comment_cache.get(&id) {
            self.detail.comments = Some(cached.clone());
            return None;
        }
        let empty = self
            .repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .find(|i| i.id == id)
            .is_some_and(|i| i.comment_count == 0);
        if empty {
            self.comment_cache.insert(id, Vec::new());
            self.detail.comments = Some(Vec::new());
            return None;
        }
        self.detail.comments = None;
        Some(id)
    }

    /// Whether comment `c` is folded down to its header row (#210).
    pub fn is_comment_folded(&self, c: &Comment) -> bool {
        self.folded_comments.contains(&c.id)
    }

    /// `Space` / `Enter` in the detail pane: fold or unfold the selected
    /// comment. The body is not foldable, so with it selected this does
    /// nothing and returns `false`; the caller re-anchors the viewport only
    /// when it returns `true`.
    pub fn toggle_selected_comment_fold(&mut self) -> bool {
        let DetailSel::Comment(i) = self.detail.sel else {
            return false;
        };
        let Some(id) = self
            .detail
            .comments
            .as_ref()
            .and_then(|c| c.get(i))
            .map(|c| c.id.clone())
        else {
            return false;
        };
        if !self.folded_comments.remove(&id) {
            self.folded_comments.insert(id);
        }
        true
    }

    /// The selected comment's id and whether it is folded; `None` with the
    /// body selected or the thread not loaded.
    fn selected_comment_fold(&self) -> Option<(String, bool)> {
        let DetailSel::Comment(i) = self.detail.sel else {
            return None;
        };
        let c = self.detail.comments.as_ref()?.get(i)?;
        Some((c.id.clone(), self.is_comment_folded(c)))
    }

    /// `←` in the detail pane (#217) — "out one level": fold an unfolded
    /// comment; from a folded comment or the body, move focus to the tree.
    /// Returns `true` when it folded, so the caller re-anchors the viewport.
    pub fn detail_left(&mut self) -> bool {
        match self.selected_comment_fold() {
            Some((id, false)) => {
                self.folded_comments.insert(id);
                true
            }
            _ => {
                self.focus = Focus::List;
                false
            }
        }
    }

    /// `→` in the detail pane (#217) — "in one level": unfold a folded
    /// comment; anywhere else a no-op. Returns `true` when it unfolded.
    pub fn detail_right(&mut self) -> bool {
        match self.selected_comment_fold() {
            Some((id, true)) => {
                self.folded_comments.remove(&id);
                true
            }
            _ => false,
        }
    }

    /// `[` / `]` in the detail pane: fold or unfold every comment of the
    /// loaded thread. Other threads' folds are left alone.
    pub fn set_all_comments_folded(&mut self, folded: bool) {
        let Some(comments) = self.detail.comments.as_ref() else {
            return;
        };
        for c in comments {
            if folded {
                self.folded_comments.insert(c.id.clone());
            } else {
                self.folded_comments.remove(&c.id);
            }
        }
    }

    /// Record a fetched thread so revisiting the issue costs nothing.
    pub fn cache_comments(&mut self, id: String, comments: Vec<Comment>) {
        self.comment_cache.insert(id, comments);
    }

    /// Forget one thread — used after a mutation that may have changed it.
    ///
    /// Also drops the readiness judgement (#160): it was derived from this
    /// thread, so whatever changed the thread may have changed the answer.
    pub fn invalidate_comments(&mut self, id: &str) {
        self.comment_cache.remove(id);
        self.invalidate_readiness(id);
    }

    /// `→` on an issue row: move focus into the detail pane, opening the
    /// split first when it is closed. Returns the issue id when the pane
    /// was newly opened and its comments need fetching. No-op (`None`)
    /// on repo header rows — there `→` keeps meaning "expand the group".
    pub fn enter_detail(&mut self) -> Option<String> {
        let id = self.selected_issue().map(|i| i.id.clone())?;
        if self.detail.open {
            self.focus = Focus::Detail;
            None
        } else {
            self.open_detail();
            self.load_comments(id)
        }
    }

    /// `c`: start (or restart) the inline comment editor for the selected
    /// issue. Opens the detail pane first when it is closed (auto-follow —
    /// `c` works the same from the list view as it does inside the pane).
    /// Returns the issue id when the pane was newly opened and its
    /// comments need fetching, mirroring `enter_detail`.
    pub fn start_comment_editor(&mut self) -> Option<String> {
        self.selected_issue()?;
        self.editor
            .start(BodyEditor::default(), EditorTarget::NewComment);
        self.mode = Mode::CommentEditor;
        self.enter_detail()
    }

    /// `e`: edit the selected detail region — the issue body or a comment.
    /// Opens the inline editor prefilled with the current content. No-op
    /// unless the detail pane is open on an issue.
    pub fn start_edit_selected_card(&mut self) {
        if !self.detail.open {
            return;
        }
        let (body, target) = match self.detail.sel {
            DetailSel::Body => {
                let Some(issue) = self.selected_issue() else {
                    return;
                };
                (BodyEditor::from_text(&issue.body), EditorTarget::EditBody)
            }
            DetailSel::Comment(idx) => {
                let Some(c) = self
                    .detail
                    .comments
                    .as_ref()
                    .and_then(|cs| cs.get(idx))
                    .cloned()
                else {
                    return;
                };
                (
                    BodyEditor::from_text(&c.body),
                    EditorTarget::EditComment { comment_id: c.id },
                )
            }
        };
        self.editor.start(body, target);
        self.mode = Mode::CommentEditor;
    }

    /// Close the detail pane, returning focus to the list.
    pub fn close_detail(&mut self) {
        self.detail.open = false;
        self.focus = Focus::List;
        self.pr = PrState::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body of 10 rows in a 4-row viewport, and a thread of `heights` in a
    /// `view`-row comments viewport.
    fn geom(heights: &[u16], view: u16) -> DetailGeometry {
        DetailGeometry {
            body_height: 10,
            body_view: 4,
            comment_heights: heights.to_vec(),
            comments_view: view,
        }
    }

    fn at(sel: DetailSel, comments_scroll: u16) -> DetailState {
        DetailState {
            open: true,
            sel,
            comments_scroll,
            ..DetailState::default()
        }
    }

    /// `(selection, comments_scroll)` after each of `keys` steps.
    fn walk(
        d: &mut DetailState,
        g: &DetailGeometry,
        dir: isize,
        n: usize,
    ) -> Vec<(DetailSel, u16)> {
        (0..n)
            .map(|_| {
                d.step(dir, g);
                (d.sel, d.comments_scroll)
            })
            .collect()
    }

    #[test]
    fn down_through_a_folded_thread_scrolls_one_row_only_at_the_bottom_edge() {
        let g = geom(&[1; 6], 3);
        let mut d = at(DetailSel::Comment(0), 0);
        use DetailSel::Comment as C;
        assert_eq!(
            walk(&mut d, &g, 1, 6),
            vec![
                (C(1), 0),
                (C(2), 0),
                (C(3), 1),
                (C(4), 2),
                (C(5), 3),
                (C(5), 3)
            ],
            "no scroll inside the viewport, one row at the edge, no wrap"
        );
    }

    #[test]
    fn up_through_a_folded_thread_scrolls_one_row_only_at_the_top_edge() {
        let g = geom(&[1; 6], 3);
        let mut d = at(DetailSel::Comment(5), 3);
        use DetailSel::Comment as C;
        assert_eq!(
            walk(&mut d, &g, -1, 7),
            vec![
                (C(4), 3),
                (C(3), 3),
                (C(2), 2),
                (C(1), 1),
                (C(0), 0),
                (DetailSel::Body, 0),
                (DetailSel::Body, 0),
            ],
        );
    }

    #[test]
    fn a_comment_taller_than_the_viewport_scrolls_row_by_row_then_advances() {
        // Comment 1 spans rows 2..10 of an 11-row thread; viewport 4.
        let g = geom(&[2, 8, 1], 4);
        let mut d = at(DetailSel::Comment(0), 0);
        d.step(1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(1), 0));
        for want in 1..=6 {
            d.step(1, &g);
            assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(1), want));
        }
        // Bottom (row 9) now visible: the next ↓ selects comment 2 at row 10.
        d.step(1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(2), 7));

        // ↑ mirrors it: into comment 1 at its last row, then up to its header.
        d.step(-1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(1), 7));
        for want in (2..=6).rev() {
            d.step(-1, &g);
            assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(1), want));
        }
        d.step(-1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(0), 1));
    }

    #[test]
    fn body_scrolls_to_its_end_before_comment_zero_and_back() {
        let g = geom(&[1, 1], 3);
        let mut d = at(DetailSel::Body, 0);
        for want in 1..=6 {
            d.step(1, &g);
            assert_eq!((d.sel, d.body_scroll), (DetailSel::Body, want));
        }
        d.step(1, &g);
        assert_eq!(d.sel, DetailSel::Comment(0));
        d.step(-1, &g);
        assert_eq!(d.sel, DetailSel::Body);
        assert_eq!(d.body_scroll, 6, "returning keeps the body's place");
        d.step(-1, &g);
        assert_eq!(d.body_scroll, 5);
    }

    #[test]
    fn body_with_no_comments_stops_at_its_end() {
        let g = geom(&[], 3);
        let mut d = at(DetailSel::Body, 0);
        d.body_scroll = 6;
        d.step(1, &g);
        assert_eq!((d.sel, d.body_scroll), (DetailSel::Body, 6));
    }

    #[test]
    fn with_no_comments_region_the_selection_still_moves() {
        let g = geom(&[5, 5], 0);
        let mut d = at(DetailSel::Comment(0), 0);
        d.step(1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(1), 0));
        d.step(-1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(0), 0));
    }

    #[test]
    fn page_repeats_the_step_one_viewport_at_a_time() {
        let g = geom(&[1; 10], 3);
        let mut d = at(DetailSel::Comment(0), 0);
        d.page(1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(3), 1));
        d.page(-1, &g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(0), 0));
        // From the body the step count is the body's viewport (4): six rows of
        // body scroll remain, so a page stays in the body.
        let mut d = at(DetailSel::Body, 0);
        d.page(1, &g);
        assert_eq!((d.sel, d.body_scroll), (DetailSel::Body, 4));
    }

    #[test]
    fn jump_top_and_end() {
        let g = geom(&[3, 3, 3], 4);
        let mut d = at(DetailSel::Comment(1), 2);
        d.jump_end(&g);
        assert_eq!((d.sel, d.comments_scroll), (DetailSel::Comment(2), 5));
        d.body_scroll = 3;
        d.jump_top();
        assert_eq!(
            (d.sel, d.body_scroll, d.comments_scroll),
            (DetailSel::Body, 0, 0)
        );
        // No comments: the body is the last item.
        let mut d = at(DetailSel::Body, 0);
        d.jump_end(&geom(&[], 4));
        assert_eq!((d.sel, d.body_scroll), (DetailSel::Body, 6));
    }

    #[test]
    fn reveal_selected_edge_scrolls_instead_of_snapping() {
        let g = geom(&[2, 2, 2, 2], 4);
        // Comment 1 (rows 2..4) is already on screen: nothing moves.
        let mut d = at(DetailSel::Comment(1), 0);
        d.reveal_selected(&g);
        assert_eq!(d.comments_scroll, 0);
        // Comment 3 (rows 6..8) is below: just enough to show all of it.
        d.sel = DetailSel::Comment(3);
        d.reveal_selected(&g);
        assert_eq!(d.comments_scroll, 4);
    }

    #[test]
    fn fold_keeps_the_selected_header_on_its_screen_row() {
        // Comment 2's header is at row 10, drawn on screen row 2.
        let before = geom(&[5, 5, 5], 4);
        let mut d = at(DetailSel::Comment(2), 8);
        let anchor = d.fold_anchor(&before);
        assert_eq!(anchor, Some(2));
        // Comment 0 folds to one row: comment 2's header moves to row 6.
        d.after_fold(anchor, &geom(&[1, 5, 5], 4));
        assert_eq!(d.comments_scroll, 4, "header still on screen row 2");
    }

    #[test]
    fn fold_clamps_when_the_thread_gets_shorter_than_the_viewport() {
        let mut d = at(DetailSel::Body, 11);
        let anchor = d.fold_anchor(&geom(&[5, 5, 5], 4));
        assert_eq!(anchor, None);
        d.after_fold(anchor, &geom(&[1, 1, 1], 4));
        assert_eq!(d.comments_scroll, 0);
    }

    #[test]
    fn folding_a_comment_read_past_its_header_brings_it_back_on_screen() {
        // Mid-way through comment 1 (rows 2..12): its header is 4 rows above.
        let mut d = at(DetailSel::Comment(1), 6);
        let anchor = d.fold_anchor(&geom(&[2, 10, 2], 4));
        assert_eq!(anchor, Some(-4));
        d.after_fold(anchor, &geom(&[2, 1, 2], 4));
        assert_eq!(
            d.comments_scroll, 1,
            "folded header visible, clamped to max"
        );
    }
}
