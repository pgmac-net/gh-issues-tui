use super::super::prelude::*;
use super::shared::*;

pub(crate) fn next_form_field(idx: usize) -> usize {
    if idx >= ISSUE_FORM_CANCEL_ROW {
        0
    } else {
        idx + 1
    }
}

/// Previous field in the new-issue form, wrapping from title to Cancel.
pub(crate) fn prev_form_field(idx: usize) -> usize {
    if idx == 0 {
        ISSUE_FORM_CANCEL_ROW
    } else {
        idx - 1
    }
}

/// Keys for the inline new-issue form (`Mode::IssueForm`).
pub(crate) fn handle_issue_form_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(form) = &mut app.issue_form else {
        app.mode = Mode::Normal;
        return;
    };
    match key.code {
        KeyCode::Esc => {
            app.cancel_issue_form();
            app.status = Some("issue creation cancelled".into());
            return;
        }
        KeyCode::Tab => {
            form.field_idx = next_form_field(form.field_idx);
            return;
        }
        KeyCode::BackTab => {
            form.field_idx = prev_form_field(form.field_idx);
            return;
        }
        _ => {}
    }

    let idx = form.field_idx;
    match idx {
        0 => {
            if key.code == KeyCode::Enter {
                form.field_idx = next_form_field(idx);
            } else {
                apply_input_editor_key(&mut form.title, key);
            }
        }
        1 => {
            apply_body_editor_key(&mut form.body, key, form_desc_wrap_width());
        }
        _ if IssueForm::is_multi_field(idx) => {
            if key.code == KeyCode::Enter {
                let opts = form.field_options(idx);
                app.multi_selected = form.multi_set(idx).clone();
                app.start_picker(opts, 0);
                app.mode = Mode::IssueFormMulti(idx);
            }
        }
        _ if IssueForm::is_select_field(idx) => {
            if key.code == KeyCode::Enter {
                // "—" (none) is prepended; stored choices are offset by 1.
                let mut opts = form.field_options(idx);
                opts.insert(0, "\u{2014}".to_string());
                let initial = form.get_single(idx).map_or(0, |i| i + 1);
                app.start_picker(opts, initial);
                app.mode = Mode::IssueFormSelect(idx);
            }
        }
        idx if idx == ISSUE_FORM_CREATE_ROW => {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                submit_issue_form(app, client, tx);
            }
        }
        _ => {
            // Cancel row.
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                app.cancel_issue_form();
                app.status = Some("issue creation cancelled".into());
            }
        }
    }
}

/// Keys shared by every option picker: ↑/↓ navigation over the filtered
/// view, Home/End, and type-ahead filter editing (chars append, Backspace
/// deletes, Ctrl+U clears). Space is passed through when `space_filters`
/// is false so the multi-select picker can use it to toggle. Returns true
/// Single-select picker for a form field (`Mode::IssueFormSelect`).
pub(crate) fn handle_form_select_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::IssueForm,
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                if let Some(form) = &mut app.issue_form {
                    // Index 0 is "—" (clear); real options are offset by 1.
                    form.set_single(idx, orig.checked_sub(1));
                }
                app.mode = Mode::IssueForm;
            }
            // No options at all → close; filter matching nothing → no-op
            // so the filter can be corrected.
            None if app.select_options.is_empty() => app.mode = Mode::IssueForm,
            None => {}
        },
        _ => {}
    }
}

pub(crate) fn handle_form_multi_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::IssueForm, // discard toggles
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker_selected_original()
                && !app.multi_selected.remove(&orig)
            {
                app.multi_selected.insert(orig);
            }
        }
        KeyCode::Enter => {
            if let Some(form) = &mut app.issue_form {
                *form.multi_set_mut(idx) = app.multi_selected.clone();
            }
            app.mode = Mode::IssueForm;
        }
        _ => {}
    }
}

/// Create the issue described by the form, then close it.
pub(crate) fn submit_issue_form(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(form) = &app.issue_form else { return };
    if form.options.is_none() {
        app.status = Some("still loading repo options — try again in a moment".into());
        return;
    }
    let Some(params) = form.build_params() else {
        app.status = Some("a title is required".into());
        return;
    };
    let repo = form.repo.clone();
    app.cancel_issue_form();
    app.status = Some(format!("creating issue in {repo}…"));
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let msg = match client.create_issue(&params).await {
            Ok((number, _url)) => AppEvent::MutationDone(format!("created #{number} in {repo}")),
            Err(e) => AppEvent::MutationFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::provider::types::FormOptions;

    #[test]
    fn tab_and_back_tab_wrap_across_all_form_fields_and_buttons() {
        let mut app = issue_form_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        assert_eq!(app.issue_form.as_ref().unwrap().field_idx, 0);
        handle_issue_form_key(&mut app, key(KeyCode::BackTab), &client, &tx);
        assert_eq!(
            app.issue_form.as_ref().unwrap().field_idx,
            ISSUE_FORM_CANCEL_ROW,
            "Shift+Tab from the first field wraps to Cancel"
        );
        handle_issue_form_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(
            app.issue_form.as_ref().unwrap().field_idx,
            0,
            "Tab from Cancel wraps back to title"
        );

        for _ in 0..=ISSUE_FORM_CANCEL_ROW {
            handle_issue_form_key(&mut app, key(KeyCode::Tab), &client, &tx);
        }
        assert_eq!(
            app.issue_form.as_ref().unwrap().field_idx,
            0,
            "a full lap of Tab returns to the title field"
        );
    }

    #[test]
    fn title_field_edits_inline_and_enter_advances_focus() {
        let mut app = issue_form_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        for c in "hello".chars() {
            handle_issue_form_key(&mut app, key(KeyCode::Char(c)), &client, &tx);
        }
        assert_eq!(app.issue_form.as_ref().unwrap().title.buffer, "hello");

        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);
        assert_eq!(
            app.issue_form.as_ref().unwrap().field_idx,
            1,
            "Enter on the title field moves focus to description, not a newline"
        );
    }

    #[test]
    fn description_field_enter_inserts_newline_inline() {
        let mut app = issue_form_test_app();
        app.issue_form.as_mut().unwrap().field_idx = 1;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        for c in "line one".chars() {
            handle_issue_form_key(&mut app, key(KeyCode::Char(c)), &client, &tx);
        }
        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);
        for c in "line two".chars() {
            handle_issue_form_key(&mut app, key(KeyCode::Char(c)), &client, &tx);
        }

        let form = app.issue_form.unwrap();
        assert_eq!(form.body.text(), "line one\nline two");
        assert_eq!(
            form.field_idx, 1,
            "Enter in the description field must not move focus"
        );
    }

    #[tokio::test]
    async fn create_row_enter_submits_when_valid() {
        let mut app = issue_form_test_app();
        {
            let form = app.issue_form.as_mut().unwrap();
            form.title.start("a new issue");
            form.field_idx = ISSUE_FORM_CREATE_ROW;
            form.options = Some(FormOptions {
                repo_id: "R_repo".into(),
                labels: Vec::new(),
                users: Vec::new(),
                milestones: Vec::new(),
                projects: Vec::new(),
                issue_types: Vec::new(),
            });
        }
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert!(
            app.issue_form.is_none(),
            "a valid submission (title set, options loaded) tears the form down \
             immediately, before the create mutation is spawned"
        );
    }

    #[test]
    fn cancel_row_enter_and_space_discard_the_form() {
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        for cancel_key in [KeyCode::Enter, KeyCode::Char(' ')] {
            let mut app = issue_form_test_app();
            app.issue_form.as_mut().unwrap().field_idx = ISSUE_FORM_CANCEL_ROW;
            handle_issue_form_key(&mut app, key(cancel_key), &client, &tx);
            assert!(app.issue_form.is_none());
            assert_eq!(app.mode, Mode::Normal);
        }
    }

    #[test]
    fn esc_cancels_regardless_of_focused_field() {
        let mut app = issue_form_test_app();
        app.issue_form.as_mut().unwrap().field_idx = 1;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_issue_form_key(&mut app, key(KeyCode::Esc), &client, &tx);
        assert!(app.issue_form.is_none());
        assert_eq!(app.mode, Mode::Normal);
    }
}
