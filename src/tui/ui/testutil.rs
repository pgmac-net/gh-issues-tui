//! Fixtures and screen-scraping helpers shared by the rendering tests.
//!
//! These read geometry back out of a rendered buffer rather than recomputing
//! it from the constants the renderer used — a test that repeats the
//! production arithmetic proves only that the arithmetic was copied.

use super::draw;
use super::popups::draw_confirm_popup;
use super::prelude::*;
use crate::provider::types::{IssueState, Label};
use crate::tui::app::Mode;

pub(super) fn issue(labels: Vec<Label>) -> Issue {
    Issue {
        id: "id".into(),
        number: 114,
        title: "Upgrade Calico".into(),
        body: String::new(),
        state: IssueState::Open,
        url: String::new(),
        author: String::new(),
        assignees: vec![],
        labels,
        comment_count: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        closed_at: None,
    }
}

pub(super) fn test_comment(body: &str) -> Comment {
    Comment {
        id: "c".into(),
        author: "octocat".into(),
        created_at: chrono::Utc::now(),
        body: body.into(),
    }
}

/// Single-repo app with one issue in `state`, selected, `Mode::ConfirmState`.
pub(super) fn confirm_app(state: IssueState) -> App {
    use crate::provider::types::RepoIssues;

    let mut i = issue(vec![]);
    i.state = state;
    let mut app = App::new(
        "org".into(),
        None,
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    );
    app.state_filter = crate::tui::app::StateFilter::All;
    app.set_data(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![i],
    }]);
    app.selected = 1; // 0 = repo header, 1 = the issue
    app.mode = super::Mode::ConfirmState;
    app
}

pub(super) fn render_confirm_buffer(app: &App) -> ratatui::buffer::Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| draw_confirm_popup(f, app, &Theme::default()))
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(super) fn rendered_confirm_popup(app: &App) -> String {
    render_confirm_buffer(app)
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// True if the cell at the start of `needle`'s first match is drawn
/// reversed-video (the focused-button style).
pub(super) fn is_reversed_at(buf: &ratatui::buffer::Buffer, needle: &str) -> bool {
    let width = buf.area().width;
    let content: String = buf.content().iter().map(|c| c.symbol()).collect();
    let byte_idx = content.find(needle).expect("needle not found in buffer");
    let cell_idx = content[..byte_idx].chars().count();
    let x = (cell_idx as u16) % width;
    let y = (cell_idx as u16) / width;
    buf[(x, y)]
        .modifier
        .contains(ratatui::style::Modifier::REVERSED)
}

// Characterisation goldens (issue #87): fixtures shared across the
// rendering tests.

/// A bare app with no data loaded, for tests that only need a `Mode` set.
pub(super) fn test_app() -> App {
    App::new(
        "org".into(),
        None,
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    )
}

/// Render the whole UI — mode dispatch included — into a `TestBackend`.
pub(super) fn render_app(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    use crate::tui::harness::HarnessRegistry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| draw(f, app, &Theme::default(), &HarnessRegistry::default()))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// A bordered box found in the rendered buffer, with its rows as strings.
#[derive(Debug)]
pub(super) struct PopupBox {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) rows: Vec<String>,
}

impl PopupBox {
    /// The whole box as one string, for substring assertions.
    pub(super) fn text(&self) -> String {
        self.rows.join("\n")
    }
}

pub(super) fn cell_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> String {
    buf[(x, y)].symbol().to_string()
}

/// Locate the topmost overlay popup: the bordered box whose top-left
/// corner sits furthest right. Overlays here are centred and narrower
/// than whatever they cover, so the most-indented corner is the one
/// drawn last. The list pane's own corner at x = 0 is never a candidate.
pub(super) fn popup_box(buf: &ratatui::buffer::Buffer) -> PopupBox {
    let area = *buf.area();
    let mut best: Option<(u16, u16)> = None;
    for y in 0..area.height {
        for x in 1..area.width {
            if cell_at(buf, x, y) == "┌" && best.is_none_or(|(bx, _)| x > bx) {
                best = Some((x, y));
            }
        }
    }
    let (x, y) = best.expect("no popup box found in buffer");

    let mut width = 1;
    while x + width < area.width && cell_at(buf, x + width, y) != "┐" {
        width += 1;
    }
    width += 1; // include the closing corner

    let mut height = 1;
    while y + height < area.height && cell_at(buf, x, y + height) != "└" {
        height += 1;
    }
    height += 1; // include the bottom border row

    let rows = (y..y + height)
        .map(|row| (x..x + width).map(|col| cell_at(buf, col, row)).collect())
        .collect();

    PopupBox {
        x,
        y,
        width,
        height,
        rows,
    }
}

/// An app sitting in `mode` with a loaded picker: three options behind a
/// leading clear entry, the second real option highlighted.
pub(super) fn picker_app(mode: Mode) -> App {
    let mut app = test_app();
    app.picker.start(
        vec!["—".into(), "alpha".into(), "beta".into(), "gamma".into()],
        2,
    );
    app.mode = mode;
    app
}

/// A single-issue app with the detail pane open, for geometry assertions.
pub(super) fn detail_app() -> App {
    use crate::provider::types::RepoIssues;

    let mut i = issue(vec![]);
    i.body = (1..=40).map(|n| format!("body {n}\n")).collect();
    let mut app = test_app();
    app.state_filter = crate::tui::app::StateFilter::All;
    app.set_data(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![i],
    }]);
    app.selected = 1;
    app.open_detail();
    app.detail.comments = Some(vec![test_comment("first"), test_comment("second")]);
    app
}

/// Column of the detail pane's left border, found by scanning the top row
/// for the second box corner.
pub(super) fn detail_pane_x(buf: &ratatui::buffer::Buffer) -> u16 {
    (1..buf.area().width)
        .find(|&x| cell_at(buf, x, 0) == "┌")
        .expect("detail pane border not found")
}

/// Row of the horizontal rule separating the body region from the
/// comments region, i.e. the body region's bottom border.
pub(super) fn detail_region_split_y(buf: &ratatui::buffer::Buffer, pane_x: u16) -> u16 {
    (1..buf.area().height)
        .find(|&y| cell_at(buf, pane_x, y) == "└")
        .expect("body region bottom border not found")
}
