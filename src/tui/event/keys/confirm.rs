use super::super::prelude::*;

pub(crate) fn handle_confirm_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
            app.confirm_choice = match app.confirm_choice {
                ConfirmChoice::Yes => ConfirmChoice::No,
                ConfirmChoice::No => ConfirmChoice::Yes,
            };
        }
        KeyCode::Char('y') => confirm_toggle_state(app, client, tx),
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.status = Some("cancelled".into());
        }
        KeyCode::Enter => match app.confirm_choice {
            ConfirmChoice::Yes => confirm_toggle_state(app, client, tx),
            ConfirmChoice::No => {
                app.mode = Mode::Normal;
                app.status = Some("cancelled".into());
            }
        },
        _ => {}
    }
}

/// Applies the close/reopen mutation and returns to `Mode::Normal`. Shared
/// by the `y` shortcut and Enter-on-Yes in `handle_confirm_key`.
pub(crate) fn confirm_toggle_state(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    app.mode = Mode::Normal;
    let target = match app.selected_issue() {
        Some(i) => match i.state {
            IssueState::Open => IssueState::Closed,
            IssueState::Closed => IssueState::Open,
        },
        None => return,
    };
    let msg = match target {
        IssueState::Closed => "issue closed",
        IssueState::Open => "issue reopened",
    };
    with_issue(
        app,
        client,
        tx,
        msg,
        CommentRefresh::Skip,
        move |c, id| async move { c.set_state(&id, target).await },
    );
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn confirm_arrow_and_tab_toggle_focus() {
        let (mut app, client, tx) = confirm_test_app();

        handle_confirm_key(&mut app, key(KeyCode::Right), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);
        assert_eq!(
            app.mode,
            Mode::ConfirmState,
            "toggling focus must not close the popup"
        );

        handle_confirm_key(&mut app, key(KeyCode::Left), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);

        handle_confirm_key(&mut app, key(KeyCode::Char('h')), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Char('l')), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);
    }

    #[test]
    fn confirm_enter_on_no_cancels_without_mutating() {
        let (mut app, client, tx) = confirm_test_app();
        let original_state = app.selected_issue().unwrap().state;

        handle_confirm_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("cancelled"));
        assert_eq!(app.selected_issue().unwrap().state, original_state);
    }

    #[tokio::test]
    async fn confirm_enter_on_yes_triggers_mutation() {
        let (mut app, client, tx) = confirm_test_app();
        app.confirm_choice = ConfirmChoice::Yes;

        handle_confirm_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("working…"));
    }

    #[tokio::test]
    async fn confirm_y_shortcut_triggers_mutation_regardless_of_focus() {
        let (mut app, client, tx) = confirm_test_app();
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Char('y')), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("working…"));
    }

    #[test]
    fn confirm_n_and_esc_shortcuts_cancel_regardless_of_focus() {
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let (mut app, client, tx) = confirm_test_app();
            app.confirm_choice = ConfirmChoice::Yes;

            handle_confirm_key(&mut app, key(code), &client, &tx);

            assert_eq!(app.mode, Mode::Normal);
            assert_eq!(app.status.as_deref(), Some("cancelled"));
        }
    }
}
