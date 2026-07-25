use super::super::prelude::*;
use super::shared::*;

pub(crate) fn handle_pr_picker_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.close_pr_picker(),
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                let pr = app.pr_links[orig].clone();
                app.open_pr_summary(pr.clone());
                spawn_pr_summary(client, pr, tx);
            }
            None if app.select_options.is_empty() => app.close_pr_picker(),
            None => {}
        },
        _ => {}
    }
}

/// The PR summary popup's navigable rows at the live terminal width. Read off
/// the same row model the popup draws, so a target's row index is exactly the
pub(crate) fn handle_pr_summary_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_summary(),
        KeyCode::Char('j') | KeyCode::Down => {
            app.pr_scroll = app.pr_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.pr_scroll = app.pr_scroll.saturating_sub(1);
        }
        KeyCode::Tab => {
            let targets = pr_targets(app);
            app.select_pr_target(1, &targets);
        }
        KeyCode::BackTab => {
            let targets = pr_targets(app);
            app.select_pr_target(-1, &targets);
        }
        KeyCode::Char('o') | KeyCode::Enter => {
            if let Some(url) = app.pr_selected_url(&pr_targets(app)) {
                match open::that(&url) {
                    Ok(()) => app.status = Some(format!("opened {url}")),
                    Err(e) => app.status = Some(format!("open failed: {e}")),
                }
            }
        }
        KeyCode::Char('r') => {
            if let Some(pr) = app.pr_target.clone() {
                app.refresh_pr_summary();
                spawn_pr_summary(client, pr, tx);
            }
        }
        _ => {}
    }
}
