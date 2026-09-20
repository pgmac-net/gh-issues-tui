//! Keys for `Mode::ConfirmPriority` — the confirmation in front of a
//! set-priority write that would strip labels an inferred rank identified
//! (#162, `docs/adr/0003-*`).
//!
//! The convention path never reaches here: a repo with any `priority:*` label
//! commits straight from the picker, exactly as it did before inference
//! existed. Modelled on `move_issue::handle_confirm_move_key`.

use super::super::prelude::*;

pub(crate) fn handle_confirm_priority_key(
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
        KeyCode::Char('y') => confirm_priority(app, client, tx),
        KeyCode::Char('n') | KeyCode::Esc => cancel_priority(app),
        KeyCode::Enter => match app.confirm_choice {
            ConfirmChoice::Yes => confirm_priority(app, client, tx),
            ConfirmChoice::No => cancel_priority(app),
        },
        _ => {}
    }
}

fn cancel_priority(app: &mut App) {
    app.pending_priority = None;
    app.mode = Mode::Normal;
    app.status = Some("cancelled".into());
}

/// Writes the label set captured when the picker committed. Shared by the `y`
/// shortcut and Enter-on-Yes.
fn confirm_priority(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    app.mode = Mode::Normal;
    let Some(pending) = app.pending_priority.take() else {
        return;
    };
    // The label set was captured at picker-commit time, so a refetch while
    // this popup was open must not let it land on a different issue — the
    // names the user just read would then be the wrong issue's labels.
    let still_target = app
        .selected_issue()
        .is_some_and(|i| i.id == pending.issue_id);
    if !still_target {
        app.status = Some("selection changed \u{2014} priority not set".into());
        return;
    }
    let (org, repo) = match app.selected_repo() {
        Some(r) => (app.org.clone(), r.repo.clone()),
        None => return,
    };
    let names = pending.names;
    with_issue(
        app,
        client,
        tx,
        "priority updated",
        CommentRefresh::Skip,
        move |c, id| async move { c.set_labels(&id, &repo, &org, &names).await },
    );
}

#[cfg(test)]
mod tests {
    use super::super::detail::handle_priority_set_key;
    use super::super::testutil::*;
    use super::*;

    use crate::tui::event::handle_app_event;

    /// `app_with_issue` plus session ranks, the way real ones arrive.
    fn ranked_app(labels: &[&str], ranks: &[(&str, Option<u8>)]) -> (App, String) {
        let (mut app, id) = app_with_issue(labels);
        app.merge_label_ranks(ranks.iter().map(|(k, v)| (k.to_string(), *v)).collect());
        (app, id)
    }

    fn tx() -> mpsc::UnboundedSender<AppEvent> {
        mpsc::unbounded_channel::<AppEvent>().0
    }

    /// Open the inferred picker on `options`, ready for an Enter.
    fn open_picker(app: &mut App, id: &str, options: &[&str], idx: usize) {
        app.picker.priority_issue = Some(id.to_string());
        app.picker
            .start(options.iter().map(|o| o.to_string()).collect(), idx);
        app.mode = Mode::PrioritySet;
    }

    // ---- which options the picker offers ----

    #[test]
    fn a_repo_using_the_convention_offers_only_convention_labels() {
        let (mut app, id) = ranked_app(&["bug"], &[("P0", Some(4))]);
        app.picker.priority_issue = Some(id.clone());
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityOptions {
                issue_id: id,
                result: Ok(vec![
                    repo_label("L1", "bug"),
                    repo_label("L2", "P0"),
                    repo_label("L3", "priority:high"),
                ]),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::PrioritySet);
        assert_eq!(
            app.picker.options,
            vec!["\u{2014}".to_string(), "priority:high".to_string()],
            "a ranked P0 must not join a convention repo's options"
        );
    }

    #[test]
    fn without_a_convention_the_repos_ranked_labels_are_offered() {
        let (mut app, id) = ranked_app(&["bug"], &[("P0", Some(4)), ("P1", Some(3))]);
        app.picker.priority_issue = Some(id.clone());
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityOptions {
                issue_id: id,
                result: Ok(vec![
                    repo_label("L1", "bug"),
                    repo_label("L2", "P0"),
                    repo_label("L3", "P1"),
                ]),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::PrioritySet);
        assert_eq!(
            app.picker.options,
            vec!["\u{2014}".to_string(), "P1".to_string(), "P0".to_string()]
        );
    }

    #[test]
    fn with_no_ranker_and_nothing_ranked_the_picker_says_what_it_said_before() {
        let (mut app, id) = app_with_issue(&["bug"]);
        app.picker.priority_issue = Some(id.clone());
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityOptions {
                issue_id: id,
                result: Ok(vec![repo_label("L1", "bug"), repo_label("L2", "P0")]),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.priority_issue.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("no priority:* labels on this repo")
        );
    }

    // ---- the picker's own rank request ----

    #[test]
    fn landing_ranks_open_the_picker_over_them() {
        let (mut app, id) = app_with_issue(&["bug"]);
        app.picker.priority_issue = Some(id.clone());
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityRanks {
                issue_id: id,
                labels: vec![repo_label("L1", "bug"), repo_label("L2", "P0")],
                result: Ok([("P0".to_string(), Some(4)), ("bug".to_string(), None)].into()),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::PrioritySet);
        assert_eq!(
            app.picker.options,
            vec!["\u{2014}".to_string(), "P0".to_string()]
        );
    }

    #[test]
    fn ranks_landing_after_the_selection_moved_are_kept_but_open_nothing() {
        let (mut app, id) = app_with_issue(&["bug"]);
        app.picker.priority_issue = Some(id.clone());
        app.selected = 0; // header row: selected_issue() is now None
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityRanks {
                issue_id: id,
                labels: vec![repo_label("L2", "P0")],
                result: Ok([("P0".to_string(), Some(4))].into()),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.priority_issue.is_none());
        assert_eq!(
            app.label_rank.rank_of("P0"),
            Some(4),
            "the answer cost a request; sorting can still use it"
        );
    }

    #[test]
    fn a_failed_rank_request_turns_inference_off_for_the_session() {
        let (mut app, id) = app_with_issue(&["bug"]);
        app.picker.priority_issue = Some(id.clone());
        let client = test_client();

        handle_app_event(
            &mut app,
            AppEvent::PriorityRanks {
                issue_id: id,
                labels: vec![repo_label("L2", "P0")],
                result: Err("TypeSafe returned 429 Too Many Requests".into()),
            },
            &client,
            None,
            &tx(),
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.priority_issue.is_none());
        assert!(app.label_rank.has_failed());
        assert_eq!(
            app.status.as_deref(),
            Some("priorities failed: TypeSafe returned 429 Too Many Requests")
        );
    }

    // ---- committing from the picker ----

    #[test]
    fn a_write_that_removes_a_ranked_label_asks_first() {
        let (mut app, id) = ranked_app(
            &["blocker", "bug"],
            &[("blocker", Some(4)), ("bug", None), ("P1", Some(3))],
        );
        open_picker(&mut app, &id, &["\u{2014}", "P1"], 1);
        let client = test_client();

        handle_priority_set_key(&mut app, key(KeyCode::Enter), &client, &tx());

        assert_eq!(app.mode, Mode::ConfirmPriority);
        let pending = app.pending_priority.as_ref().expect("pending priority");
        assert_eq!(pending.pick.as_deref(), Some("P1"));
        assert_eq!(pending.removes, vec!["blocker".to_string()]);
        assert_eq!(pending.names, vec!["bug".to_string(), "P1".to_string()]);
        assert_eq!(
            app.confirm_choice,
            ConfirmChoice::No,
            "Enter without looking must not delete a label"
        );
    }

    #[tokio::test]
    async fn a_write_that_removes_nothing_goes_straight_through() {
        let (mut app, id) = ranked_app(&["bug"], &[("bug", None), ("P0", Some(4))]);
        open_picker(&mut app, &id, &["\u{2014}", "P0"], 1);
        let client = test_client();

        handle_priority_set_key(&mut app, key(KeyCode::Enter), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.pending_priority.is_none(),
            "nothing to lose, so nothing to confirm"
        );
    }

    #[tokio::test]
    async fn the_convention_path_never_confirms_and_keeps_ranked_labels() {
        // `blocker` is ranked, but this repo says what it means, so the
        // write strips `priority:*` only and asks nothing (#162, AC3).
        let (mut app, id) = ranked_app(
            &["blocker", "priority:low"],
            &[("blocker", Some(4)), ("priority:low", None)],
        );
        open_picker(&mut app, &id, &["\u{2014}", "priority:high"], 1);
        // What the convention path writes: `blocker` survives, because only
        // `priority:*` labels are stripped.
        assert_eq!(
            priority_label_set(
                app.selected_issue().expect("selected"),
                Some("priority:high")
            ),
            vec!["blocker".to_string(), "priority:high".to_string()]
        );
        let client = test_client();

        handle_priority_set_key(&mut app, key(KeyCode::Enter), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.pending_priority.is_none(),
            "the convention path must not route through the confirmation"
        );
    }

    #[test]
    fn the_rank_word_is_decoration_and_never_filters_or_writes() {
        let (mut app, id) = ranked_app(&["bug"], &[("P1", Some(3))]);
        open_picker(&mut app, &id, &["\u{2014}", "P1"], 1);

        app.picker.filter = "high".into();
        assert!(
            app.picker.filtered().is_empty(),
            "the rank word must not be in the options the filter searches"
        );
        assert_eq!(app.picker.options[1], "P1", "options hold the label name");
    }

    // ---- the confirmation ----

    fn pending_app() -> (App, Provider) {
        let (mut app, id) = ranked_app(&["blocker", "bug"], &[("blocker", Some(4))]);
        app.pending_priority = Some(PendingPriority {
            issue_id: id,
            pick: Some("P1".into()),
            removes: vec!["blocker".into()],
            names: vec!["bug".into(), "P1".into()],
        });
        app.confirm_choice = ConfirmChoice::No;
        app.mode = Mode::ConfirmPriority;
        (app, test_client())
    }

    #[test]
    fn confirm_arrow_and_tab_toggle_focus() {
        let (mut app, client) = pending_app();
        for code in [
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Tab,
            KeyCode::Char('h'),
            KeyCode::Char('l'),
        ] {
            let before = app.confirm_choice;
            handle_confirm_priority_key(&mut app, key(code), &client, &tx());
            assert_ne!(app.confirm_choice, before, "{code:?} did not toggle");
            assert_eq!(app.mode, Mode::ConfirmPriority, "{code:?} left the popup");
        }
    }

    #[test]
    fn enter_on_no_cancels_without_writing() {
        let (mut app, client) = pending_app();
        handle_confirm_priority_key(&mut app, key(KeyCode::Enter), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_priority.is_none());
        assert_eq!(app.status.as_deref(), Some("cancelled"));
    }

    #[test]
    fn esc_cancels_without_writing() {
        let (mut app, client) = pending_app();
        handle_confirm_priority_key(&mut app, key(KeyCode::Esc), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_priority.is_none());
    }

    #[tokio::test]
    async fn yes_writes_the_set_captured_when_the_picker_committed() {
        let (mut app, client) = pending_app();
        handle_confirm_priority_key(&mut app, key(KeyCode::Char('y')), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_priority.is_none());
    }

    #[test]
    fn a_selection_change_under_the_popup_aborts_the_write() {
        let (mut app, client) = pending_app();
        app.selected = 0; // header row: selected_issue() is now None

        handle_confirm_priority_key(&mut app, key(KeyCode::Char('y')), &client, &tx());

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pending_priority.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("selection changed \u{2014} priority not set")
        );
    }
}
