//! Keys for the repo workflows view's filter menu and field pickers (#266).
//! Movement and type-ahead go through the shared picker table (#233); this
//! decides only what `Enter` and `Esc` do.

use std::time::Instant;

use super::super::prelude::*;
use super::shared::*;
use crate::tui::app::RunFilterField;

pub(crate) fn handle_run_filter_menu_key(app: &mut App, key: KeyEvent) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => app.run_filter_menu_pick(Instant::now()),
        _ => {}
    }
}

pub(crate) fn handle_run_filter_pick_key(app: &mut App, key: KeyEvent, field: RunFilterField) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        // Back to the menu, which re-reads the current values.
        KeyCode::Esc => app.open_run_filter_menu(),
        KeyCode::Enter => app.run_filter_pick(field, Instant::now()),
        _ => {}
    }
}
