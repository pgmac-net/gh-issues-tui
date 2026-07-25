use super::super::prelude::*;
use super::shared::*;

pub(crate) fn handle_input_key(
    app: &mut App,
    key: KeyEvent,
    kind: InputKind,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Esc => {
            app.mode = match kind {
                InputKind::FilterField(_) => Mode::FilterMenu,
                _ => Mode::Normal,
            };
        }
        KeyCode::Enter => {
            let value = app.input.buffer.clone();
            app.mode = Mode::Normal;
            submit_input(app, kind, value, client, tx);
        }
        _ => {
            apply_input_editor_key(&mut app.input, key);
        }
    }
}

pub(crate) fn submit_input(
    app: &mut App,
    kind: InputKind,
    value: String,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match kind {
        InputKind::Search | InputKind::FilterField(_) => {
            app.apply_filter_input(kind, &value);
            if matches!(kind, InputKind::FilterField(_)) {
                app.mode = Mode::FilterMenu;
            }
        }
        InputKind::Assignees => {
            let logins = split_csv(&value);
            with_issue(
                app,
                client,
                tx,
                "assignees updated",
                move |c, id| async move { c.set_assignees(&id, &logins).await },
            );
        }
        InputKind::Title => {
            if value.trim().is_empty() {
                app.status = Some("empty title discarded".into());
                return;
            }
            with_issue(app, client, tx, "title updated", move |c, id| async move {
                c.update_title(&id, &value).await
            });
        }
        InputKind::Org => {
            let org = value.trim().to_string();
            if org.is_empty() || org.eq_ignore_ascii_case(&app.org) {
                app.status = Some("org unchanged".into());
                return;
            }
            app.status = Some(format!("switching to {org}…"));
            app.switch_org(org);
            spawn_fetch(client, app, tx);
        }
        InputKind::GotoNumber => {
            let trimmed = value.trim().trim_start_matches('#').trim();
            match trimmed.parse::<u64>() {
                Ok(number) => nav(app, client, tx, |a| {
                    a.jump_to_number(number);
                }),
                Err(_) => app.status = Some("not an issue number".into()),
            }
        }
    }
}
