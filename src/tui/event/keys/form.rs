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
                app.picker.multi_selected = form.multi_set(idx).clone();
                app.picker.start(opts, 0);
                app.mode = Mode::IssueFormMulti(idx);
            }
        }
        // The project list could not be fetched (#253): nothing to pick from.
        6 if form.projects_unavailable() => {}
        _ if IssueForm::is_select_field(idx) => {
            if key.code == KeyCode::Enter {
                // "—" (none) is prepended; stored choices are offset by 1.
                let mut opts = form.field_options(idx);
                opts.insert(0, "\u{2014}".to_string());
                let initial = form.get_single(idx).map_or(0, |i| i + 1);
                app.picker.start(opts, initial);
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
        KeyCode::Enter => match app.picker.selected_original() {
            Some(orig) => {
                if let Some(form) = &mut app.issue_form {
                    // Index 0 is "—" (clear); real options are offset by 1.
                    form.set_single(idx, orig.checked_sub(1));
                }
                app.mode = Mode::IssueForm;
            }
            // No options at all → close; filter matching nothing → no-op
            // so the filter can be corrected.
            None if app.picker.options.is_empty() => app.mode = Mode::IssueForm,
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
            if let Some(orig) = app.picker.selected_original() {
                app.picker.toggle_multi(orig);
            }
        }
        KeyCode::Enter => {
            if let Some(form) = &mut app.issue_form {
                *form.multi_set_mut(idx) = app.picker.multi_selected.clone();
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
    if form.options_error.is_some() {
        app.status = Some("can't create: repo options failed \u{2014} Esc to close".into());
        return;
    }
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
        // The created issue goes straight into the list, no refetch (#249).
        let msg = match client.create_issue(&params).await {
            Ok(issue) => AppEvent::IssueCreated {
                repo,
                issue: Box::new(issue),
            },
            Err(crate::provider::error::ProviderError::CreatedUnread { number }) => {
                AppEvent::CreatedUnread { repo, number }
            }
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

    fn options_without_projects(why: &str) -> FormOptions {
        FormOptions {
            repo_id: "R_repo".into(),
            projects_unavailable: Some(why.into()),
            ..FormOptions::default()
        }
    }

    #[test]
    fn the_project_row_says_why_it_is_empty() {
        let mut app = issue_form_test_app();
        let form = app.issue_form.as_mut().unwrap();
        form.options = Some(options_without_projects("token lacks read:project"));
        assert!(form.projects_unavailable());
        assert_eq!(
            form.field_display(6),
            "unavailable \u{2014} token lacks read:project"
        );
    }

    #[test]
    fn enter_on_an_unavailable_project_row_opens_nothing() {
        let mut app = issue_form_test_app();
        let form = app.issue_form.as_mut().unwrap();
        form.options = Some(options_without_projects("token lacks read:project"));
        form.field_idx = 6;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::IssueForm, "no picker opened");
    }

    #[test]
    fn a_working_project_list_still_opens_its_picker() {
        let mut app = issue_form_test_app();
        let form = app.issue_form.as_mut().unwrap();
        form.options = Some(FormOptions {
            repo_id: "R_repo".into(),
            projects: vec![crate::provider::types::IdName {
                id: "P_1".into(),
                name: "Homelab".into(),
            }],
            ..FormOptions::default()
        });
        form.field_idx = 6;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::IssueFormSelect(6));
    }

    #[test]
    fn create_says_the_options_failed_instead_of_still_loading() {
        let mut app = issue_form_test_app();
        let form = app.issue_form.as_mut().unwrap();
        form.title.start("a new issue");
        form.field_idx = ISSUE_FORM_CREATE_ROW;
        form.options_error = Some("API errors: boom".into());
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_issue_form_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert!(app.issue_form.is_some(), "the form stays open");
        assert_eq!(
            app.status.as_deref(),
            Some("can't create: repo options failed \u{2014} Esc to close")
        );
    }

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
                projects_unavailable: None,
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
