//! Keys for `Mode::MovePicker` and `Mode::ConfirmMove` — picking a target
//! repo for the selected issue, then confirming the transfer.

use super::super::prelude::*;
use super::shared::*;

pub(crate) fn handle_move_picker_key(app: &mut App, key: KeyEvent) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            let Some(orig) = app.picker.selected_original() else {
                // Filter matches nothing: keep the picker open unless it is
                // truly empty (mirrors the other single-select pickers).
                if app.picker.options.is_empty() {
                    app.mode = Mode::Normal;
                }
                return;
            };
            let target = app.picker.options[orig].clone();
            let Some(issue_id) = app.selected_issue().map(|i| i.id.clone()) else {
                app.mode = Mode::Normal;
                return;
            };
            app.pending_move = Some(PendingMove { issue_id, target });
            app.confirm_choice = ConfirmChoice::No;
            app.mode = Mode::ConfirmMove;
        }
        _ => {}
    }
}

pub(crate) fn handle_confirm_move_key(
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
        KeyCode::Char('y') => confirm_move(app, client, tx),
        KeyCode::Char('n') | KeyCode::Esc => cancel_move(app),
        KeyCode::Enter => match app.confirm_choice {
            ConfirmChoice::Yes => confirm_move(app, client, tx),
            ConfirmChoice::No => cancel_move(app),
        },
        _ => {}
    }
}

fn cancel_move(app: &mut App) {
    app.pending_move = None;
    app.mode = Mode::Normal;
    app.status = Some("cancelled".into());
}

/// Applies the move mutation and returns to `Mode::Normal`. Shared by the
/// `y` shortcut and Enter-on-Yes in `handle_confirm_move_key`.
fn confirm_move(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    app.mode = Mode::Normal;
    let Some(pending) = app.pending_move.take() else {
        return;
    };
    // The picker captured the issue id at commit time; a refetch or
    // selection change while the confirm popup was open must not let this
    // mutation land on a different issue.
    let still_target = app
        .selected_issue()
        .is_some_and(|i| i.id == pending.issue_id);
    if !still_target {
        app.status = Some("selection changed — issue not moved".into());
        return;
    }
    let org = app.org.clone();
    let target = pending.target.clone();
    let msg = format!("moved to {target}");
    with_issue(
        app,
        client,
        tx,
        msg,
        CommentRefresh::Skip,
        move |c, id| async move { c.move_issue(&id, &org, &target).await },
    );
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    fn move_test_app() -> (App, Provider, mpsc::UnboundedSender<AppEvent>) {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.repos.push(crate::provider::types::RepoIssues {
            repo: "other-repo".into(),
            repo_url: "u".into(),
            issues: vec![],
        });
        app.rebuild_rows();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        (app, client, tx)
    }

    #[test]
    fn move_targets_excludes_the_issues_own_repo() {
        let (app, _client, _tx) = move_test_app();
        assert_eq!(app.move_targets(), vec!["other-repo".to_string()]);
    }

    #[test]
    fn move_targets_empty_when_org_has_only_one_repo() {
        let (app, _id) = app_with_issue(&[]);
        assert!(app.move_targets().is_empty());
    }

    #[test]
    fn enter_on_a_target_opens_the_confirm_popup() {
        let (mut app, _client, _tx) = move_test_app();
        let issue_id = app.selected_issue().unwrap().id.clone();
        app.open_move_picker(app.move_targets());

        handle_move_picker_key(&mut app, key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::ConfirmMove);
        assert_eq!(
            app.pending_move,
            Some(PendingMove {
                issue_id,
                target: "other-repo".into(),
            })
        );
    }

    #[test]
    fn esc_on_the_picker_cancels_without_setting_a_pending_move() {
        let (mut app, _client, _tx) = move_test_app();
        app.open_move_picker(app.move_targets());

        handle_move_picker_key(&mut app, key(KeyCode::Esc));

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_move.is_none());
    }

    #[test]
    fn confirm_enter_on_no_cancels_without_mutating() {
        let (mut app, client, tx) = move_test_app();
        let issue_id = app.selected_issue().unwrap().id.clone();
        app.pending_move = Some(PendingMove {
            issue_id,
            target: "other-repo".into(),
        });
        app.mode = Mode::ConfirmMove;
        app.confirm_choice = ConfirmChoice::No;

        handle_confirm_move_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("cancelled"));
        assert!(app.pending_move.is_none());
    }

    #[tokio::test]
    async fn confirm_enter_on_yes_triggers_the_mutation() {
        let (mut app, client, tx) = move_test_app();
        let issue_id = app.selected_issue().unwrap().id.clone();
        app.pending_move = Some(PendingMove {
            issue_id,
            target: "other-repo".into(),
        });
        app.mode = Mode::ConfirmMove;
        app.confirm_choice = ConfirmChoice::Yes;

        handle_confirm_move_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("working…"));
    }

    #[test]
    fn confirm_drops_a_stale_pending_move_whose_issue_scrolled_away() {
        let (mut app, client, tx) = move_test_app();
        app.pending_move = Some(PendingMove {
            issue_id: "I_gone".into(),
            target: "other-repo".into(),
        });
        app.mode = Mode::ConfirmMove;
        app.confirm_choice = ConfirmChoice::Yes;

        handle_confirm_move_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.status.as_deref(),
            Some("selection changed — issue not moved")
        );
        assert!(app.pending_move.is_none());
    }
}
