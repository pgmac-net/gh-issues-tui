//! Per-mode key handlers. `handle_key` dispatches on `App::mode`; each
//! submodule owns the modes named after it.

use super::prelude::*;

pub(crate) mod confirm;
pub(crate) mod detail;
pub(crate) mod filter;
pub(crate) mod form;
pub(crate) mod input;
pub(crate) mod normal;
pub(crate) mod pr;
pub(super) mod shared;
#[cfg(test)]
pub(crate) mod testutil;

use confirm::handle_confirm_key;
use detail::{handle_comment_editor_key, handle_labels_set_key, handle_priority_set_key};
use filter::{
    handle_calendar_key, handle_filter_menu_key, handle_select_field_key,
    handle_select_field_multi_key,
};
use form::{handle_form_multi_key, handle_form_select_key, handle_issue_form_key};
use input::handle_input_key;
use normal::handle_normal_key;
use pr::{handle_pr_picker_key, handle_pr_summary_key};

pub(super) fn handle_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match app.mode {
        Mode::Normal => handle_normal_key(app, key, client, tx),
        Mode::Input(kind) => handle_input_key(app, key, kind, client, tx),
        Mode::FilterMenu => handle_filter_menu_key(app, key),
        Mode::SelectField(idx) => handle_select_field_key(app, key, idx),
        Mode::SelectFieldMulti(idx) => handle_select_field_multi_key(app, key, idx),
        Mode::Calendar(idx) => handle_calendar_key(app, key, idx),
        Mode::ConfirmState => handle_confirm_key(app, key, client, tx),
        Mode::IssueForm => handle_issue_form_key(app, key, client, tx),
        Mode::IssueFormSelect(idx) => handle_form_select_key(app, key, idx),
        Mode::IssueFormMulti(idx) => handle_form_multi_key(app, key, idx),
        Mode::CommentEditor => handle_comment_editor_key(app, key, client, tx),
        Mode::PrioritySet => handle_priority_set_key(app, key, client, tx),
        Mode::LabelsSet => handle_labels_set_key(app, key, client, tx),
        Mode::PrPicker => handle_pr_picker_key(app, key, client, tx),
        Mode::PrSummary => handle_pr_summary_key(app, key, client, tx),
        Mode::Help => app.mode = Mode::Normal,
    }
}
