use super::super::prelude::*;
use super::detail::{detail_page_rows, detail_scroll, snap_after_select};

pub(crate) fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Char('q') => {
            if app.detail.open {
                app.close_detail();
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Esc if app.detail.open => app.close_detail(),
        // In the detail pane Tab/Shift+Tab move between comments; from the
        // list they switch into the pane.
        KeyCode::Tab => {
            if app.focus == Focus::Detail {
                app.detail.select(1);
                snap_after_select(app);
            } else {
                app.cycle_focus();
            }
        }
        KeyCode::BackTab => {
            if app.focus == Focus::Detail {
                app.detail.select(-1);
                snap_after_select(app);
            } else {
                app.cycle_focus();
            }
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('r') => {
            app.loading = true;
            app.status = Some("reloading…".into());
            spawn_fetch(client, app, tx);
        }

        // navigation
        KeyCode::Char('j') | KeyCode::Down => {
            if app.focus == Focus::Detail {
                detail_scroll(app, 1);
            } else {
                nav(app, client, tx, |a| a.move_selection(1));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.focus == Focus::Detail {
                detail_scroll(app, -1);
            } else {
                nav(app, client, tx, |a| a.move_selection(-1));
            }
        }
        KeyCode::PageDown => {
            if app.focus == Focus::Detail {
                let page = detail_page_rows(app);
                detail_scroll(app, page);
            } else {
                nav(app, client, tx, |a| a.move_selection(15));
            }
        }
        KeyCode::PageUp => {
            if app.focus == Focus::Detail {
                let page = detail_page_rows(app);
                detail_scroll(app, -page);
            } else {
                nav(app, client, tx, |a| a.move_selection(-15));
            }
        }
        KeyCode::Char('g') | KeyCode::Home => nav(app, client, tx, |a| a.selected = 0),
        KeyCode::Char('G') | KeyCode::End => {
            nav(app, client, tx, |a| {
                a.selected = a.rows.len().saturating_sub(1);
            });
        }

        // grouping (list focus only — in the detail pane ← focuses the list)
        KeyCode::Right if app.focus == Focus::List => {
            if app.selected_issue().is_some() {
                // Issue row: → goes deeper into the detail pane (mirror of
                // ← backing out), opening the split like Enter if needed.
                if let Some(issue_id) = app.enter_detail() {
                    spawn_comments(client, issue_id, tx);
                }
            } else {
                app.set_current_collapsed(false);
            }
        }
        KeyCode::Left => {
            if app.focus == Focus::Detail {
                app.focus = Focus::List;
            } else {
                nav(app, client, tx, |a| a.set_current_collapsed(true));
            }
        }
        KeyCode::Char(' ') => nav(app, client, tx, App::toggle_collapse),
        KeyCode::Char('[') => nav(app, client, tx, |a| a.set_all_collapsed(true)),
        KeyCode::Char(']') => nav(app, client, tx, |a| a.set_all_collapsed(false)),

        // filters & sort
        KeyCode::Char('/') => {
            app.input.start(&app.filters.text.clone());
            app.mode = Mode::Input(InputKind::Search);
        }
        // jump the selection to an issue by number (does not filter the list)
        KeyCode::Char('#') => {
            app.input.start("");
            app.mode = Mode::Input(InputKind::GotoNumber);
        }
        KeyCode::Char('f') => {
            app.state_filter = app.state_filter.next();
            if app.state_filter != StateFilter::Open && !app.include_closed {
                // Closed issues were never fetched; upgrade the dataset once.
                app.include_closed = true;
                app.loading = true;
                app.status = Some("fetching closed issues…".into());
                spawn_fetch(client, app, tx);
            }
            app.rebuild_rows();
            app.expand_single_visible();
        }
        KeyCode::Char('F') => {
            app.filter_menu_idx = 0;
            app.mode = Mode::FilterMenu;
        }
        KeyCode::Char('s') => {
            app.sort_key = app.sort_key.next();
            app.rebuild_rows();
        }
        KeyCode::Char('S') => {
            app.sort_desc = !app.sort_desc;
            app.rebuild_rows();
        }

        // switch org/owner
        KeyCode::Char('w') => {
            let current = app.org.clone();
            app.input.start(&current);
            app.mode = Mode::Input(InputKind::Org);
        }

        // open in browser
        KeyCode::Char('o') => {
            let url = app
                .selected_issue()
                .map(|i| i.url.clone())
                .or_else(|| app.selected_repo().map(|r| r.repo_url.clone()));
            if let Some(url) = url {
                match open::that(&url) {
                    Ok(()) => app.status = Some(format!("opened {url}")),
                    Err(e) => app.status = Some(format!("open failed: {e}")),
                }
            }
        }
        // copy short reference to clipboard (via OSC 52 — works over SSH,
        // no system clipboard dependency)
        KeyCode::Char('y') => {
            if let Some(r) = app.selected_short_ref() {
                match osc52_copy(&r) {
                    Ok(()) => app.status = Some(format!("copied {r}")),
                    Err(e) => app.status = Some(format!("copy failed: {e}")),
                }
            }
        }
        KeyCode::Char('O') => {
            if let Some(url) = app.selected_repo().map(|r| r.repo_url.clone()) {
                match open::that(&url) {
                    Ok(()) => app.status = Some(format!("opened {url}")),
                    Err(e) => app.status = Some(format!("open failed: {e}")),
                }
            }
        }

        // detail
        KeyCode::Enter => {
            if let Some(issue_id) = app.selected_issue().map(|i| i.id.clone()) {
                app.open_detail();
                spawn_comments(client, issue_id, tx);
            } else {
                app.toggle_collapse();
            }
        }

        // mutations
        KeyCode::Char('c') => {
            if let Some(issue_id) = app.start_comment_editor() {
                spawn_comments(client, issue_id, tx);
            }
        }
        // Edit the highlighted detail card: the issue body or a comment.
        KeyCode::Char('e') if app.detail.open => app.start_edit_selected_card(),
        KeyCode::Char('x') => {
            if app.selected_issue().is_some() {
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmState;
            }
        }
        KeyCode::Char('a') => {
            if let Some(issue) = app.selected_issue() {
                let current = issue.assignees.join(", ");
                app.input.start(&current);
                app.mode = Mode::Input(InputKind::Assignees);
            }
        }
        KeyCode::Char('l') => {
            let target = app
                .selected_issue()
                .map(|i| i.id.clone())
                .zip(app.selected_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.picker.label_issue = Some(issue_id.clone());
                app.status = Some("loading labels…".into());
                spawn_label_options(client, app.org.clone(), repo, issue_id, tx);
            }
        }
        KeyCode::Char('t') => {
            if let Some(issue) = app.selected_issue() {
                let title = issue.title.clone();
                app.input.start(&title);
                app.mode = Mode::Input(InputKind::Title);
            }
        }
        KeyCode::Char('p') => {
            let target = app
                .selected_issue()
                .map(|i| i.id.clone())
                .zip(app.selected_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.picker.priority_issue = Some(issue_id.clone());
                app.status = Some("loading priorities…".into());
                spawn_priority_options(client, app.org.clone(), repo, issue_id, tx);
            }
        }
        KeyCode::Char('n') => {
            if let Some(repo) = app.selected_repo().map(|r| r.repo.clone()) {
                app.open_issue_form(repo.clone());
                spawn_form_options(client, app.org.clone(), repo, tx);
            }
        }
        KeyCode::Char('P') if app.detail.open => {
            if !client.supports_pr_summary() {
                app.status = Some("PR summaries not supported by this provider".into());
                return;
            }
            let links = app.collect_pr_links();
            match links.len() {
                0 => app.status = Some("no PR links found".into()),
                1 => {
                    let pr = links.into_iter().next().expect("checked len == 1");
                    app.open_pr_summary(pr.clone());
                    spawn_pr_summary(client, pr, tx);
                }
                _ => app.open_pr_picker(links),
            }
        }
        _ => {}
    }
}
