//! Screen geometry, computed once.
//!
//! Both the renderer and the key handler need to know where the panes are:
//! `ui::draw` to place widgets, `event.rs` to clamp scrolling to a region's
//! viewport. These used to be worked out twice — `ui::draw` with `Layout`,
//! `event::detail_metrics` by restating the same percentages and border
//! insets by hand, kept in agreement by a comment. This module is the one
//! place that answer comes from.
//!
//! Everything here is a pure function of a `Rect`, so nothing depends on a
//! draw having happened first and draw code stays free of state mutation.
//! [`from_terminal_size`] is the single point that reads the real terminal.

use ratatui::layout::{Constraint, Layout, Rect};

/// Body region's share of the detail pane height (percent); the comments
/// region takes the rest.
const DETAIL_BODY_PCT: u16 = 45;
/// Minimum outer height for the body region so its metadata header stays
/// visible even in a short terminal.
const DETAIL_BODY_MIN_H: u16 = 6;
/// The list pane's share of the width when the detail pane is open.
const LIST_PCT: u16 = 40;
const DETAIL_PCT: u16 = 60;

/// The frame split into the main area and the two single-row status lines.
pub struct FrameAreas {
    pub main: Rect,
    pub info: Rect,
    pub bottom: Rect,
}

/// The main area split into the issue list and, when open, the detail pane.
pub struct PaneAreas {
    pub list: Rect,
    /// `None` when the detail pane is closed — the list has the whole width.
    pub detail: Option<Rect>,
}

/// The detail pane split into its two independently scrolling regions.
pub struct DetailAreas {
    pub body: Rect,
    /// `None` when the pane is too short to host a second bordered region.
    pub comments: Option<Rect>,
}

/// Split the whole frame: main area above the info and status lines.
pub fn frame(area: Rect) -> FrameAreas {
    let [main, info, bottom] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    FrameAreas { main, info, bottom }
}

/// Split the main area into the list and detail panes.
pub fn panes(main: Rect, detail_open: bool) -> PaneAreas {
    if !detail_open {
        return PaneAreas {
            list: main,
            detail: None,
        };
    }
    let [list, detail] = Layout::horizontal([
        Constraint::Percentage(LIST_PCT),
        Constraint::Percentage(DETAIL_PCT),
    ])
    .areas(main);
    PaneAreas {
        list,
        detail: Some(detail),
    }
}

/// Split the detail pane's height into `(body, comments)` outer heights, each
/// including its own border rows.
pub fn detail_split(area_h: u16) -> (u16, u16) {
    // Too short to host two bordered regions: give it all to the body.
    if area_h <= DETAIL_BODY_MIN_H + 3 {
        return (area_h, 0);
    }
    let body = ((area_h as u32 * DETAIL_BODY_PCT as u32 / 100) as u16).max(DETAIL_BODY_MIN_H);
    // Always leave the comments region at least three rows.
    let body = body.min(area_h.saturating_sub(3));
    (body, area_h - body)
}

/// Split the detail pane into its body and comments regions.
pub fn detail_regions(detail: Rect) -> DetailAreas {
    let (body_h, comments_h) = detail_split(detail.height);
    let [body, comments] =
        Layout::vertical([Constraint::Length(body_h), Constraint::Length(comments_h)])
            .areas(detail);
    DetailAreas {
        body,
        comments: (comments_h > 0).then_some(comments),
    }
}

/// The harness pane, the identity row above it and the key row below it.
pub struct HarnessAreas {
    /// Whose session this is: launcher, issue and live state (#132). Zero
    /// height when the terminal is too short to spare it.
    pub header: Rect,
    /// Where the child's screen is drawn; also the size its PTY is set to.
    pub pane: Rect,
    pub status: Rect,
}

/// Split the frame for a harness session (#23): the child gets everything
/// except the two chrome rows. A session is drawn full-frame rather than into
/// the detail split because an agent's own TUI needs the room, and because it
/// keeps the PTY size a plain function of the terminal.
///
/// Every chrome row is a row taken off the child — `pane` is what reaches
/// `PtySize` — so the rows are shed as the terminal shrinks, header first: it
/// is the one that repeats what the key row cannot.
pub fn harness_areas(area: Rect) -> HarnessAreas {
    let empty = |y: u16| Rect::new(area.x, y, area.width, 0);
    // Too short to spare a row: the child keeps the lot. Anything that gives
    // the pane zero rows would be handed to `PtySize`, which the OS rejects.
    if area.height <= 1 {
        return HarnessAreas {
            header: empty(area.y),
            pane: area,
            status: empty(area.y),
        };
    }
    if area.height == 2 {
        // One row of chrome is affordable, and the keys are the half a user
        // cannot recover by looking — the header only names what they just
        // opened.
        let [pane, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        return HarnessAreas {
            header: empty(area.y),
            pane,
            status,
        };
    }
    let [header, pane, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    HarnessAreas {
        header,
        pane,
        status,
    }
}

/// The text width inside a bordered area — its width less both border columns.
pub fn inner_width(area: Rect) -> u16 {
    area.width.saturating_sub(2)
}

/// The text height inside a bordered area — its height less both border rows.
pub fn inner_height(area: Rect) -> u16 {
    area.height.saturating_sub(2)
}

/// The detail pane's inner text width for a frame `frame_width` columns wide.
/// Used by everything that wraps text into that pane — the renderer, the
/// comment editor's visual-row navigation, and the scroll clamps.
pub fn detail_inner_width(frame_width: u16) -> u16 {
    let full = Rect::new(0, 0, frame_width, 1);
    match panes(full, true).detail {
        Some(detail) => inner_width(detail),
        None => 0,
    }
}

/// The live terminal as a `Rect`, falling back to 80×24 when the size cannot
/// be read. The one place the real terminal is consulted, so every other
/// function here stays pure and testable at any size.
pub fn from_terminal_size() -> Rect {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    Rect::new(0, 0, cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_reserves_two_status_rows() {
        let f = frame(Rect::new(0, 0, 100, 30));
        assert_eq!(f.main.height, 28);
        assert_eq!(f.info.height, 1);
        assert_eq!(f.bottom.height, 1);
        assert_eq!(f.info.y, 28);
        assert_eq!(f.bottom.y, 29);
    }

    #[test]
    fn closed_detail_gives_the_list_the_whole_width() {
        let p = panes(Rect::new(0, 0, 100, 28), false);
        assert_eq!(p.list.width, 100);
        assert!(p.detail.is_none());
    }

    #[test]
    fn open_detail_splits_forty_sixty() {
        let p = panes(Rect::new(0, 0, 100, 28), true);
        let detail = p.detail.expect("detail pane");
        assert_eq!(p.list.width, 40);
        assert_eq!(detail.x, 40);
        assert_eq!(detail.width, 60);
    }

    #[test]
    fn panes_cover_the_main_area_without_gaps_at_any_width() {
        for width in [37u16, 80, 81, 99, 100, 201] {
            let main = Rect::new(0, 0, width, 20);
            let p = panes(main, true);
            let detail = p.detail.expect("detail pane");
            assert_eq!(
                p.list.width + detail.width,
                width,
                "panes must tile the main area at width {width}"
            );
            assert_eq!(detail.x, p.list.width, "no gap at width {width}");
        }
    }

    #[test]
    fn detail_inner_width_matches_the_drawn_pane() {
        for width in [37u16, 80, 81, 100, 201] {
            let detail = panes(Rect::new(0, 0, width, 20), true)
                .detail
                .expect("detail pane");
            assert_eq!(detail_inner_width(width), inner_width(detail), "at {width}");
        }
    }

    #[test]
    fn detail_split_gives_body_and_comments_regions() {
        let (body, comments) = detail_split(30);
        assert!(body > 0 && comments > 0);
        assert_eq!(body + comments, 30);
        // The body gets its 45% share once there is room for both regions.
        assert_eq!(body, 13);
    }

    #[test]
    fn detail_split_collapses_the_comments_region_when_too_short() {
        let (body, comments) = detail_split(6);
        assert_eq!((body, comments), (6, 0));
    }

    #[test]
    fn detail_split_floors_the_body_and_reserves_three_comment_rows() {
        // Just past the collapse threshold: the body's floor applies, and the
        // comments region keeps its three-row minimum.
        let (body, comments) = detail_split(10);
        assert_eq!(body, DETAIL_BODY_MIN_H);
        assert_eq!(comments, 4);

        let (body, comments) = detail_split(11);
        assert!(comments >= 3, "comments region kept at least three rows");
        assert_eq!(body + comments, 11);
    }

    #[test]
    fn detail_regions_tile_the_pane() {
        let detail = Rect::new(40, 0, 60, 28);
        let r = detail_regions(detail);
        let comments = r.comments.expect("comments region");
        assert_eq!(r.body.y, 0);
        assert_eq!(comments.y, r.body.height);
        assert_eq!(r.body.height + comments.height, 28);
    }

    #[test]
    fn detail_regions_reports_no_comments_region_when_collapsed() {
        assert!(detail_regions(Rect::new(0, 0, 60, 6)).comments.is_none());
    }

    #[test]
    fn harness_areas_give_the_child_everything_but_the_two_chrome_rows() {
        let a = harness_areas(Rect::new(0, 0, 120, 40));
        assert_eq!(a.header.height, 1);
        assert_eq!(a.header.y, 0);
        assert_eq!(a.pane.height, 38);
        assert_eq!(a.pane.width, 120);
        assert_eq!(a.pane.y, 1);
        assert_eq!(a.status.height, 1);
        assert_eq!(a.status.y, 39);
    }

    #[test]
    fn harness_pane_never_collapses_to_zero_rows() {
        // A one-row terminal must still yield a usable PtySize; a zero-row
        // pty is rejected by the OS.
        for height in [1u16, 2, 3, 4] {
            let a = harness_areas(Rect::new(0, 0, 80, height));
            assert!(a.pane.height >= 1, "at height {height}");
            assert_eq!(
                a.header.height + a.pane.height + a.status.height,
                height,
                "at height {height} the regions must tile the frame exactly"
            );
        }
    }

    #[test]
    fn harness_chrome_sheds_the_header_before_the_keys() {
        // Two rows buys one row of chrome. It goes to the key row: the header
        // only names the session the user just opened, while the keys are the
        // half they cannot recover by looking.
        let a = harness_areas(Rect::new(0, 0, 80, 2));
        assert_eq!(a.header.height, 0);
        assert_eq!(a.pane.height, 1);
        assert_eq!(a.status.height, 1);
    }

    #[test]
    fn harness_one_row_terminal_gives_the_child_everything() {
        let a = harness_areas(Rect::new(0, 0, 80, 1));
        assert_eq!(a.header.height, 0);
        assert_eq!(a.pane.height, 1);
        assert_eq!(a.status.height, 0);
    }

    #[test]
    fn harness_header_appears_once_a_third_row_is_available() {
        let a = harness_areas(Rect::new(0, 0, 80, 3));
        assert_eq!(a.header.height, 1);
        assert_eq!(a.pane.height, 1);
        assert_eq!(a.status.height, 1);
    }
}
