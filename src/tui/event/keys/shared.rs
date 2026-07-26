use super::super::prelude::*;

pub(crate) fn picker_common_key(app: &mut App, key: KeyEvent, space_filters: bool) -> bool {
    let visible = app.picker.filtered().len();
    match key.code {
        KeyCode::Down => {
            if visible > 0 {
                app.picker.idx = (app.picker.idx + 1) % visible;
            }
            true
        }
        KeyCode::Up => {
            if visible > 0 {
                app.picker.idx = (app.picker.idx + visible - 1) % visible;
            }
            true
        }
        KeyCode::Home => {
            app.picker.idx = 0;
            true
        }
        KeyCode::End => {
            app.picker.idx = visible.saturating_sub(1);
            true
        }
        KeyCode::Backspace => {
            app.picker.filter_backspace();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.picker.filter_clear();
            true
        }
        KeyCode::Char(' ') if !space_filters => false,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.picker.filter_push(c);
            true
        }
        _ => false,
    }
}

pub(crate) fn apply_body_editor_key(
    body: &mut BodyEditor,
    key: KeyEvent,
    wrap_width: usize,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => body.newline(),
        KeyCode::Backspace => body.backspace(),
        KeyCode::Delete => body.delete_char(),
        KeyCode::Left if ctrl => body.word_left(),
        KeyCode::Right if ctrl => body.word_right(),
        KeyCode::Left => body.left(),
        KeyCode::Right => body.right(),
        KeyCode::Up => body.up_visual(wrap_width),
        KeyCode::Down => body.down_visual(wrap_width),
        KeyCode::Home => body.home(),
        KeyCode::End => body.end(),
        KeyCode::Char('a') if ctrl => body.home(),
        KeyCode::Char('e') if ctrl => body.end(),
        KeyCode::Char('w') if ctrl => body.delete_word_back(),
        KeyCode::Char('u') if ctrl => body.kill_to_start(),
        KeyCode::Char('k') if ctrl => body.kill_to_end(),
        KeyCode::Char('d') if ctrl => body.delete_char(),
        KeyCode::Char(c) if !ctrl => body.insert(c),
        _ => return false,
    }
    true
}

/// Keys shared by every single-line `InputState`: readline-style editing.
/// Returns whether the key was consumed.
pub(crate) fn apply_input_editor_key(input: &mut InputState, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete_char(),
        KeyCode::Left if ctrl => input.word_left(),
        KeyCode::Right if ctrl => input.word_right(),
        KeyCode::Left => input.left(),
        KeyCode::Right => input.right(),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        KeyCode::Char('a') if ctrl => input.home(),
        KeyCode::Char('e') if ctrl => input.end(),
        KeyCode::Char('w') if ctrl => input.delete_word_back(),
        KeyCode::Char('u') if ctrl => input.kill_to_start(),
        KeyCode::Char('k') if ctrl => input.kill_to_end(),
        KeyCode::Char('d') if ctrl => input.delete_char(),
        KeyCode::Char(c) if !ctrl => input.insert(c),
        _ => return false,
    }
    true
}

pub(crate) fn form_desc_wrap_width() -> usize {
    let cols = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    (issue_form_width(cols) as usize).saturating_sub(ISSUE_FORM_LABEL_WIDTH)
}

/// New-issue form: `Tab`/`Shift+Tab` move between fields and the
/// Create/Cancel buttons (handled here regardless of focus); everything
/// else dispatches on the focused row — title and description edit inline,
/// choice fields open their picker popup on Enter, Create/Cancel activate
/// The wrap width the inline comment section is currently rendered at.
pub(crate) fn comment_wrap_width() -> usize {
    layout::detail_inner_width(layout::from_terminal_size().width) as usize
}

/// The detail pane's current inner width and the body/comments regions'
/// viewport heights. Asks `tui::layout` for the same regions `ui::draw`
/// places, so the scroll clamps here cannot disagree with what is drawn.
pub(crate) fn detail_metrics() -> (u16, u16, u16) {
    let frame = layout::frame(layout::from_terminal_size());
    let Some(detail) = layout::panes(frame.main, true).detail else {
        return (0, 0, 0);
    };
    let regions = layout::detail_regions(detail);
    (
        layout::inner_width(detail),
        layout::inner_height(regions.body),
        regions.comments.map_or(0, layout::inner_height),
    )
}

/// The PR summary popup's navigable rows at the live terminal width. Read
/// off the same row model the popup draws, so a target's row index is
/// exactly the row it highlights.
pub(crate) fn pr_targets(app: &App) -> Vec<crate::tui::app::PrTarget> {
    let cols = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    ui::pr_targets(app.pr.summary.as_ref(), ui::pr_summary_inner_width(cols))
}
