//! Rendering. Pure functions of `&App` — no state is mutated here.
//!
//! Split by screen area: the list pane and its status bars, the detail pane,
//! the popups, the PR summary, the new-issue form, and the small drawing
//! helpers they share.

mod detail;
mod form;
mod harness;
mod list;
mod popups;
mod pr;
mod widgets;

#[cfg(test)]
mod testutil;

pub use detail::{body_content_height, comment_height, comment_offset};
pub use pr::{pr_max_scroll, pr_summary_inner_height, pr_summary_inner_width, pr_targets};

/// Items every rendering submodule needs. Kept in one place so the split
/// files stay mechanical — each `use super::prelude::*` replaces what was a
/// single import block at the top of the old `ui.rs`.
mod prelude {
    pub use std::num::NonZeroU16;

    pub use ratatui::Frame;
    pub use ratatui::buffer::{Buffer, CellDiffOption};
    pub use ratatui::layout::{Constraint, Layout, Margin, Rect};
    pub use ratatui::style::{Color, Modifier, Style};
    pub use ratatui::text::{Line, Span};
    pub use ratatui::widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    };
    pub use unicode_width::UnicodeWidthStr;

    pub use crate::provider::types::{Comment, Issue};
    pub use crate::tui::app::App;
    pub use crate::tui::layout;
    pub use crate::tui::linkmap::{self, LinkRect};
    pub use crate::tui::theme::Theme;
}

use prelude::*;

use crate::tui::app::Mode;
use crate::tui::harness::HarnessRegistry;

/// Draw a whole frame: the panes, the status lines, then whichever popup the
/// current mode calls for.
pub fn draw(f: &mut Frame, app: &App, t: &Theme, registry: &HarnessRegistry) {
    // An attached session takes the whole frame — the list and detail pane
    // are not drawn behind it, so a full-screen agent TUI is not fighting
    // for space with a view nobody can see.
    if app.harness.active.is_some()
        && matches!(
            app.mode,
            Mode::Harness | Mode::HarnessPicker | Mode::SessionPicker | Mode::ConfirmHarness(_)
        )
    {
        harness::draw_harness(f, app, t, registry, f.area());
        draw_popup(f, app, t);
        return;
    }

    let frame = layout::frame(f.area());
    let panes = layout::panes(frame.main, app.detail.open);

    list::draw_list(f, app, t, panes.list);
    if let Some(area) = panes.detail {
        detail::draw_detail(f, app, t, area);
    }
    list::draw_info_bar(f, app, t, frame.info);
    list::draw_bottom_line(f, app, t, frame.bottom);

    draw_popup(f, app, t);
}

/// Whichever popup the current mode calls for, over whatever is behind it.
fn draw_popup(f: &mut Frame, app: &App, t: &Theme) {
    match app.mode {
        Mode::FilterMenu => popups::draw_filter_menu(f, app, t),
        Mode::SelectField(idx) => {
            popups::draw_picker(f, app, t, popups::PickerSpec::filter_field(idx, false))
        }
        Mode::SelectFieldMulti(idx) => {
            popups::draw_picker(f, app, t, popups::PickerSpec::filter_field(idx, true))
        }
        Mode::Calendar(idx) => popups::draw_calendar_popup(f, app, t, idx),
        Mode::IssueForm => form::draw_issue_form(f, app, t),
        Mode::IssueFormSelect(idx) => {
            form::draw_issue_form(f, app, t);
            popups::draw_picker(f, app, t, popups::PickerSpec::form_field(idx, false));
        }
        Mode::IssueFormMulti(idx) => {
            form::draw_issue_form(f, app, t);
            popups::draw_picker(f, app, t, popups::PickerSpec::form_field(idx, true));
        }
        Mode::Input(kind) => popups::draw_input_popup(f, app, t, kind),
        Mode::PrioritySet => popups::draw_picker(f, app, t, popups::PickerSpec::priority()),
        Mode::LabelsSet => popups::draw_picker(f, app, t, popups::PickerSpec::labels()),
        Mode::PrPicker => popups::draw_picker(f, app, t, popups::PickerSpec::pr_links()),
        Mode::PrSummary => pr::draw_pr_summary_popup(f, app, t),
        Mode::ConfirmState => popups::draw_confirm_popup(f, app, t),
        Mode::HarnessPicker => popups::draw_picker(f, app, t, popups::PickerSpec::harnesses()),
        Mode::SessionPicker => popups::draw_picker(f, app, t, popups::PickerSpec::sessions()),
        Mode::ConfirmHarness(what) => popups::draw_harness_confirm_popup(f, app, t, what),
        Mode::MovePicker => popups::draw_picker(f, app, t, popups::PickerSpec::move_target()),
        Mode::ConfirmMove => popups::draw_confirm_move_popup(f, app, t),
        // `active` is the same flag the dismiss path uses to decide where help
        // returns to (`keys/mod.rs`), and `detach()` clears it — so it is a
        // sound stand-in for "help was opened from inside a session".
        Mode::Help => popups::draw_help(f, t, app.harness.active.is_some()),
        _ => {}
    }
}
