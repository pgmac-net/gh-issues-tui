use super::super::prelude::*;
use super::detail::{detail_jump_end, detail_page, detail_select, detail_step, with_fold};
use super::harness::{HarnessCtx, open_harness_picker, open_session_picker};

pub(crate) fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    hx: &mut HarnessCtx,
) {
    match key.code {
        KeyCode::Char('q') => {
            if app.detail.open {
                app.close_detail();
            } else if app.harness.has_running() {
                // Quitting closes the PTYs, which SIGHUPs the children — so
                // say what is about to be lost rather than doing it silently.
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmHarness(HarnessConfirm::Quit);
            } else {
                app.should_quit = true;
            }
        }
        // launch a coding harness for the selected issue (#23)
        KeyCode::Char('A') => match app.launch_action(hx.settings.default_harness.as_deref()) {
            LaunchAction::NoIssue => {
                app.status = Some("select an issue to send to a harness".into());
            }
            LaunchAction::Attach(id) => {
                app.harness.attach(id);
                app.mode = Mode::Harness;
            }
            LaunchAction::ConfirmRelaunch(id) => {
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmHarness(HarnessConfirm::Relaunch(id));
            }
            LaunchAction::Spawn { issue_ref, harness } => hx.launch(app, &issue_ref, &harness),
            LaunchAction::Pick { .. } => open_harness_picker(app, hx.settings.names()),
        },
        // reattach to a detached session
        KeyCode::Char('Z') => open_session_picker(app),
        KeyCode::Esc if app.detail.open => app.close_detail(),
        // Every key below that has a meaning in both panes acts on the focused
        // pane only (#217, ADR 0006): one branch per key, split on focus, so
        // nothing reaches the pane without focus. In the detail pane Tab /
        // Shift+Tab move header to header; from the list they switch panes.
        KeyCode::Tab | KeyCode::BackTab if app.focus == Focus::Detail => {
            detail_select(app, if key.code == KeyCode::Tab { 1 } else { -1 });
        }
        KeyCode::Tab | KeyCode::BackTab => app.cycle_focus(),
        KeyCode::Char('?') => app.open_help(app.context_topic()),
        KeyCode::Char('r') => {
            app.loading = true;
            app.status = Some("reloading…".into());
            spawn_fetch(client, app, tx);
        }

        // navigation: ↑/↓ move a row cursor through whichever pane has focus
        // — tree rows, or the body and comments of the detail pane (#217).
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            Focus::Detail => detail_step(app, 1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(1)),
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            Focus::Detail => detail_step(app, -1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(-1)),
        },
        KeyCode::PageDown => match app.focus {
            Focus::Detail => detail_page(app, 1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(15)),
        },
        KeyCode::PageUp => match app.focus {
            Focus::Detail => detail_page(app, -1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(-15)),
        },
        KeyCode::Char('g') | KeyCode::Home => match app.focus {
            Focus::Detail => app.detail.jump_top(),
            Focus::List => nav(app, client, tx, |a| a.selected = 0),
        },
        KeyCode::Char('G') | KeyCode::End => match app.focus {
            Focus::Detail => detail_jump_end(app),
            Focus::List => nav(app, client, tx, |a| {
                a.selected = a.rows.len().saturating_sub(1);
            }),
        },

        // nesting: ← out one level, → in one level, Space/Enter toggle — the
        // same meaning in the tree and the detail pane (#217, ADR 0006).
        KeyCode::Right => match app.focus {
            Focus::Detail => with_fold(app, App::detail_right),
            Focus::List => tree_right(app, client, tx),
        },
        KeyCode::Left => match app.focus {
            Focus::Detail => with_fold(app, App::detail_left),
            Focus::List => nav(app, client, tx, App::tree_left),
        },
        KeyCode::Char(' ') | KeyCode::Enter => match app.focus {
            Focus::Detail => with_fold(app, App::toggle_selected_comment_fold),
            // On an issue the toggle is `→`; on a header it opens or closes it.
            Focus::List if app.selected_issue().is_some() => tree_right(app, client, tx),
            Focus::List => nav(app, client, tx, App::toggle_collapse),
        },
        // `[`/`]`: fold or unfold everything in the focused pane.
        KeyCode::Char('[') | KeyCode::Char(']') => {
            let folded = key.code == KeyCode::Char('[');
            match app.focus {
                Focus::Detail => with_fold(app, |a| {
                    a.set_all_comments_folded(folded);
                    true
                }),
                Focus::List => nav(app, client, tx, |a| a.set_all_collapsed(folded)),
            }
        }

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
            } else {
                app.status = Some(
                    "no repo selected — filter to a repo or set \"hide empty repos\" to no".into(),
                );
            }
        }
        // move the selected issue to another repo (#135)
        KeyCode::Char('m') => {
            if app.selected_issue().is_some() {
                if !client.supports_move() {
                    app.status = Some("moving issues not supported by this provider".into());
                } else {
                    let targets = app.move_targets();
                    if targets.is_empty() {
                        app.status = Some("no other repo to move this issue to".into());
                    } else {
                        app.open_move_picker(targets);
                    }
                }
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

/// `→` (and `Space`/`Enter` on an issue) in the tree (#217): one level in. On
/// an issue that is the detail pane — opened, or just focused when already
/// open so its place is kept — with the comment fetch it may need.
fn tree_right(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    let mut step = TreeRight::Handled;
    nav(app, client, tx, |a| step = a.tree_right());
    if step == TreeRight::EnterDetail
        && let Some(issue_id) = app.enter_detail()
    {
        spawn_comments(client, issue_id, tx);
    }
}
