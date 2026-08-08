use super::super::prelude::*;
use super::shared::*;

pub(crate) fn handle_priority_set_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.picker.priority_issue = None;
            app.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            let Some(orig) = app.picker.selected_original() else {
                // Filter matches nothing: keep the picker open unless it is
                // truly empty (mirrors the select-field picker).
                if app.picker.options.is_empty() {
                    app.picker.priority_issue = None;
                    app.mode = Mode::Normal;
                }
                return;
            };
            let pick = app.picker.options[orig].clone();
            // The selection cannot move while the picker is open, but the
            // issue can vanish under a refetch that landed before it opened.
            let still_target = app
                .selected_issue()
                .is_some_and(|i| app.picker.priority_issue.as_deref() == Some(i.id.as_str()));
            app.picker.priority_issue = None;
            app.mode = Mode::Normal;
            if !still_target {
                app.status = Some("selection changed — priority not set".into());
                return;
            }
            let names = priority_label_set(
                app.selected_issue().expect("checked above"),
                (pick != "\u{2014}").then_some(pick.as_str()),
            );
            let (org, repo) = match app.selected_repo() {
                Some(r) => (app.org.clone(), r.repo.clone()),
                None => return,
            };
            with_issue(
                app,
                client,
                tx,
                "priority updated",
                CommentRefresh::Skip,
                move |c, id| async move { c.set_labels(&id, &repo, &org, &names).await },
            );
        }
        _ => {}
    }
}

pub(crate) fn handle_labels_set_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.picker.label_issue = None;
            app.mode = Mode::Normal;
        }
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker.selected_original() {
                app.picker.toggle_multi(orig);
            }
        }
        KeyCode::Enter => {
            // The selection cannot move while the picker is open, but the
            // issue can vanish under a refetch that landed before it opened.
            let still_target = app
                .selected_issue()
                .is_some_and(|i| app.picker.label_issue.as_deref() == Some(i.id.as_str()));
            let mut names: Vec<String> = app
                .picker
                .multi_selected
                .iter()
                .filter_map(|&i| app.picker.options.get(i).cloned())
                .collect();
            names.sort_unstable();
            app.picker.label_issue = None;
            app.mode = Mode::Normal;
            if !still_target {
                app.status = Some("selection changed — labels not set".into());
                return;
            }
            let (org, repo) = match app.selected_repo() {
                Some(r) => (app.org.clone(), r.repo.clone()),
                None => return,
            };
            with_issue(
                app,
                client,
                tx,
                "labels updated",
                CommentRefresh::Skip,
                move |c, id| async move { c.set_labels(&id, &repo, &org, &names).await },
            );
        }
        _ => {}
    }
}

/// Keys for the inline comment/description editor (`Mode::CommentEditor`).
pub(crate) fn handle_comment_editor_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => cancel_comment(app),
        KeyCode::Char('s') if ctrl => submit_comment(app, client, tx),
        KeyCode::Tab => app.editor.focus = next_comment_focus(app.editor.focus),
        KeyCode::BackTab => app.editor.focus = prev_comment_focus(app.editor.focus),
        KeyCode::Enter | KeyCode::Char(' ') if app.editor.focus == CommentFocus::Save => {
            submit_comment(app, client, tx)
        }
        KeyCode::Enter | KeyCode::Char(' ') if app.editor.focus == CommentFocus::Cancel => {
            cancel_comment(app)
        }
        _ if app.editor.focus == CommentFocus::Editor => {
            apply_body_editor_key(&mut app.editor.body, key, comment_wrap_width());
        }
        _ => {}
    }
}

pub(crate) fn cancel_comment(app: &mut App) {
    app.editor = EditorState::default();
    app.mode = Mode::Normal;
    app.status = Some("comment discarded".into());
}

pub(crate) fn next_comment_focus(focus: CommentFocus) -> CommentFocus {
    match focus {
        CommentFocus::Editor => CommentFocus::Save,
        CommentFocus::Save => CommentFocus::Cancel,
        CommentFocus::Cancel => CommentFocus::Editor,
    }
}

pub(crate) fn prev_comment_focus(focus: CommentFocus) -> CommentFocus {
    match focus {
        CommentFocus::Editor => CommentFocus::Cancel,
        CommentFocus::Save => CommentFocus::Editor,
        CommentFocus::Cancel => CommentFocus::Save,
    }
}

pub(crate) fn submit_comment(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let (value, target) = app.editor.take();
    app.mode = Mode::Normal;
    // An empty comment is discarded; an empty description is a valid edit
    // (clearing the body).
    if value.trim().is_empty() && !matches!(target, EditorTarget::EditBody) {
        app.status = Some("empty — discarded".into());
        return;
    }
    match target {
        EditorTarget::NewComment => {
            with_issue(
                app,
                client,
                tx,
                "comment added",
                CommentRefresh::Refetch,
                move |c, id| async move { c.add_comment(&id, &value).await },
            );
        }
        EditorTarget::EditComment { comment_id } => {
            with_issue(
                app,
                client,
                tx,
                "comment updated",
                CommentRefresh::Refetch,
                move |c, id| async move { c.update_comment(&id, &comment_id, &value).await },
            );
        }
        EditorTarget::EditBody => {
            with_issue(
                app,
                client,
                tx,
                "description updated",
                CommentRefresh::Skip,
                move |c, id| async move { c.update_body(&id, &value).await },
            );
        }
    }
}

/// Scroll the selected detail region by `lines` (negative = up), clamped to
/// that region's extent: the body to its content, a comment to its own span.
pub(crate) fn detail_scroll(app: &mut App, lines: isize) {
    let (inner_w, body_view, comments_view) = detail_metrics();
    match app.detail.sel {
        DetailSel::Body => {
            let Some(issue) = app.selected_issue() else {
                return;
            };
            let content = ui::body_content_height(issue, inner_w);
            let max = content.saturating_sub(body_view);
            app.detail.scroll_body(lines, max);
        }
        DetailSel::Comment(i) => {
            let bounds = app.detail.comments.as_ref().and_then(|comments| {
                let c = comments.get(i)?;
                let top = ui::comment_offset(comments, i, inner_w);
                let height = ui::comment_height(c, inner_w);
                Some((top, top + height.saturating_sub(comments_view)))
            });
            if let Some((lo, hi)) = bounds {
                app.detail.scroll_comment(lines, lo, hi);
            }
        }
    }
}

/// The active region's viewport height, used as the PageUp/PageDown step.
pub(crate) fn detail_page_rows(app: &App) -> isize {
    let (_, body_view, comments_view) = detail_metrics();
    match app.detail.sel {
        DetailSel::Body => body_view as isize,
        DetailSel::Comment(_) => comments_view as isize,
    }
}

/// After `select_detail` lands on a comment, snap the comments viewport so
/// that comment's header sits at the top of the region.
pub(crate) fn snap_after_select(app: &mut App) {
    let DetailSel::Comment(i) = app.detail.sel else {
        return;
    };
    let (inner_w, _, _) = detail_metrics();
    let Some(comments) = app.detail.comments.as_ref() else {
        return;
    };
    let top = ui::comment_offset(comments, i, inner_w);
    app.detail.snap_comment(top);
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    use crate::tui::event::handle_app_event;

    #[test]
    fn label_options_prechecks_current_labels_and_opens_picker() {
        let (mut app, issue_id) = app_with_issue(&["bug", "priority:high"]);
        app.picker.label_issue = Some(issue_id.clone());
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![
                    repo_label("L1", "bug"),
                    repo_label("L2", "enhancement"),
                    repo_label("L3", "priority:high"),
                ]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::LabelsSet);
        assert_eq!(
            app.picker.options,
            vec![
                "bug".to_string(),
                "enhancement".to_string(),
                "priority:high".to_string()
            ]
        );
        assert_eq!(app.picker.multi_selected, [0, 2].into_iter().collect());
    }

    #[test]
    fn label_options_stale_when_selection_moved_on() {
        let (mut app, issue_id) = app_with_issue(&["bug"]);
        // Options land after the user already moved off this issue.
        app.picker.label_issue = Some(issue_id.clone());
        app.selected = 0; // header row: selected_issue() is now None
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![repo_label("L1", "bug")]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.label_issue.is_none());
    }

    #[test]
    fn label_options_empty_repo_labels_sets_status() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.picker.label_issue = Some(issue_id.clone());
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.label_issue.is_none());
        assert_eq!(app.status.as_deref(), Some("no labels on this repo"));
    }

    #[test]
    fn labels_set_esc_discards_toggles() {
        let (mut app, issue_id) = app_with_issue(&["bug"]);
        app.picker.label_issue = Some(issue_id);
        app.picker
            .start(vec!["bug".into(), "enhancement".into()], 0);
        app.picker.multi_selected = [0].into_iter().collect();
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Char(' ')), &client, &tx); // toggle enhancement on
        handle_labels_set_key(&mut app, key(KeyCode::Esc), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.label_issue.is_none());
    }

    #[test]
    fn labels_set_space_toggles_original_index_through_filter() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.picker.label_issue = Some(issue_id);
        app.picker
            .start(vec!["alpha".into(), "beta".into(), "gamma".into()], 0);
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Char('g')), &client, &tx); // filter → gamma only
        handle_labels_set_key(&mut app, key(KeyCode::Char(' ')), &client, &tx); // toggle it

        assert!(
            app.picker.multi_selected.contains(&2),
            "toggle must hit gamma's original index, got {:?}",
            app.picker.multi_selected
        );
    }

    #[test]
    fn labels_set_enter_without_target_issue_reports_stale() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        ); // no data, no selected issue
        app.picker.label_issue = Some("I_ghost".into());
        app.picker.start(vec!["bug".into()], 0);
        app.picker.multi_selected = [0].into_iter().collect();
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.picker.label_issue.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("selection changed — labels not set")
        );
    }

    #[test]
    fn tab_cycles_comment_focus_editor_save_cancel() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        assert_eq!(app.editor.focus, CommentFocus::Editor);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.editor.focus, CommentFocus::Save);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.editor.focus, CommentFocus::Cancel);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.editor.focus, CommentFocus::Editor);
    }

    #[test]
    fn back_tab_cycles_comment_focus_in_reverse() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::BackTab), &client, &tx);
        assert_eq!(app.editor.focus, CommentFocus::Cancel);
    }

    #[test]
    fn enter_on_cancel_focus_discards_and_returns_to_normal() {
        let mut app = comment_editor_test_app();
        app.editor.body.insert('x');
        app.editor.focus = CommentFocus::Cancel;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.editor.body.text(), "");
        assert_eq!(app.status.as_deref(), Some("comment discarded"));
    }

    #[test]
    fn typed_chars_only_reach_editor_when_editor_focused() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        app.editor.focus = CommentFocus::Save;
        handle_comment_editor_key(&mut app, key(KeyCode::Char('x')), &client, &tx);
        assert_eq!(app.editor.body.text(), "");

        app.editor.focus = CommentFocus::Editor;
        handle_comment_editor_key(&mut app, key(KeyCode::Char('x')), &client, &tx);
        assert_eq!(app.editor.body.text(), "x");
    }

    #[test]
    fn esc_discards_regardless_of_focus() {
        let mut app = comment_editor_test_app();
        app.editor.body.insert('x');
        app.editor.focus = CommentFocus::Save;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::Esc), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.editor.body.text(), "");
    }
}
