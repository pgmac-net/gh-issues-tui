use super::prelude::*;

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

    /// Scroll the body region, clamped to `[0, max]` (max = content height
    /// minus the region's viewport height, 0 when it all fits).
    pub fn scroll_body(&mut self, delta: isize, max: u16) {
        let next = (self.body_scroll as isize + delta).clamp(0, max as isize);
        self.body_scroll = next as u16;
    }

    /// Scroll within the selected comment: `comments_scroll` is an absolute
    /// offset into the stacked comments paragraph, clamped to `[lo, hi]` where
    /// `lo` is the comment's top and `hi` is `lo + height − viewport` (floored
    /// at `lo`, so a comment that fits doesn't scroll).
    pub fn scroll_comment(&mut self, delta: isize, lo: u16, hi: u16) {
        let hi = hi.max(lo);
        let next = (self.comments_scroll as isize + delta).clamp(lo as isize, hi as isize);
        self.comments_scroll = next as u16;
    }

    /// Snap the comments viewport so comment `top` (a precomputed offset) sits
    /// at the top of the region. Called after `select` lands on a comment.
    pub fn snap_comment(&mut self, top: u16) {
        self.comments_scroll = top;
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
            Some(id)
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
