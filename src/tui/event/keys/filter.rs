use super::super::prelude::*;
use super::shared::*;
use chrono::TimeDelta;

pub(crate) fn handle_filter_menu_key(app: &mut App, key: KeyEvent) {
    use crate::tui::app::FILTER_FIELDS;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Char('j') | KeyCode::Down => {
            app.filter_menu_idx = (app.filter_menu_idx + 1) % FILTER_FIELDS.len();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.filter_menu_idx =
                (app.filter_menu_idx + FILTER_FIELDS.len() - 1) % FILTER_FIELDS.len();
        }
        KeyCode::Char('c') => {
            app.clear_filters();
            app.rebuild_rows();
            app.expand_single_visible();
        }
        KeyCode::Enter => {
            let idx = app.filter_menu_idx;
            if idx == crate::tui::app::FILTER_HIDE_EMPTY_IDX {
                app.toggle_hide_empty();
            } else if App::is_multi_select_field(idx) {
                let options = app.compute_multi_options(idx);
                let current = if idx == 4 {
                    &app.filters.priority
                } else {
                    &app.filters.status
                };
                app.multi_selected = options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| current.iter().any(|c| c.eq_ignore_ascii_case(o)))
                    .map(|(i, _)| i)
                    .collect();
                app.start_picker(options, 0);
                app.mode = Mode::SelectFieldMulti(idx);
            } else if App::is_select_field(idx) {
                let options = app.compute_select_options(idx);
                let current = app.current_filter_value(idx);
                let initial = options.iter().position(|v| v == &current).unwrap_or(0);
                app.start_picker(options, initial);
                app.mode = Mode::SelectField(idx);
            } else if App::is_calendar_field(idx) {
                app.calendar_init(idx);
                app.mode = Mode::Calendar(idx);
            } else {
                let current = app.current_filter_value(idx);
                app.input.start(&current);
                app.mode = Mode::Input(InputKind::FilterField(idx));
            }
        }
        _ => {}
    }
}

pub(crate) fn handle_select_field_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::FilterMenu,
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                let raw = app.select_options[orig].clone();
                let value = if raw == "\u{2014}" {
                    String::new()
                } else {
                    raw
                };
                app.apply_filter_input(InputKind::FilterField(idx), &value);
                app.mode = Mode::FilterMenu;
            }
            // No options at all → close; filter matching nothing → no-op
            // so the filter can be corrected.
            None if app.select_options.is_empty() => app.mode = Mode::FilterMenu,
            None => {}
        },
        _ => {}
    }
}

pub(crate) fn handle_select_field_multi_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::FilterMenu, // discard toggles
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker_selected_original()
                && !app.multi_selected.remove(&orig)
            {
                app.multi_selected.insert(orig);
            }
        }
        KeyCode::Enter => {
            let mut picked: Vec<usize> = app.multi_selected.iter().copied().collect();
            picked.sort();
            let values: Vec<String> = picked
                .into_iter()
                .filter_map(|i| app.select_options.get(i).cloned())
                .collect();
            app.apply_multi_filter(idx, values);
            app.mode = Mode::FilterMenu;
        }
        _ => {}
    }
}

pub(crate) fn handle_calendar_key(app: &mut App, key: KeyEvent, idx: usize) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::FilterMenu,
        KeyCode::Left => {
            app.calendar_cursor = app
                .calendar_cursor
                .pred_opt()
                .unwrap_or(app.calendar_cursor);
        }
        KeyCode::Right => {
            app.calendar_cursor = app
                .calendar_cursor
                .succ_opt()
                .unwrap_or(app.calendar_cursor);
        }
        KeyCode::Up => {
            app.calendar_cursor -= TimeDelta::days(7);
        }
        KeyCode::Down => {
            app.calendar_cursor += TimeDelta::days(7);
        }
        KeyCode::PageUp => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            app.calendar_cursor = first
                .pred_opt()
                .and_then(|d| d.with_day(1))
                .unwrap_or(first);
        }
        KeyCode::PageDown => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            let next_first = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
            };
            app.calendar_cursor = next_first;
        }
        KeyCode::Home => {
            app.calendar_cursor =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap_or(app.calendar_cursor);
        }
        KeyCode::End => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            let next_first = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
            };
            app.calendar_cursor = next_first.pred_opt().unwrap_or(next_first);
        }
        KeyCode::Enter => {
            let value = app.calendar_cursor.format("%Y-%m-%d").to_string();
            app.apply_filter_input(InputKind::FilterField(idx), &value);
            app.mode = Mode::FilterMenu;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::tui::event::keys::form::handle_form_multi_key;

    #[test]
    fn typing_filters_and_arrows_navigate_filtered_view() {
        let mut app = picker_test_app();
        assert!(picker_common_key(&mut app, key(KeyCode::Char('a')), true));
        // "a" matches alpha/beta/gamma... narrow further.
        assert!(picker_common_key(&mut app, key(KeyCode::Char('m')), true));
        assert_eq!(app.select_filter, "am");
        assert_eq!(app.picker_selected_original(), Some(2)); // gamma

        assert!(picker_common_key(&mut app, key(KeyCode::Backspace), true));
        assert!(picker_common_key(&mut app, key(KeyCode::Down), true));
        assert_eq!(app.select_filter, "a");
        // filter "a" matches all three; Down moved 0 → 1.
        assert_eq!(app.picker_selected_original(), Some(1));
    }

    #[test]
    fn multi_picker_space_toggles_original_index_through_filter() {
        let mut app = picker_test_app();
        app.issue_form = Some(IssueForm::new("alpha".into()));
        app.mode = Mode::IssueFormMulti(3);

        handle_form_multi_key(&mut app, key(KeyCode::Char('g')), 3); // filter → gamma only
        handle_form_multi_key(&mut app, key(KeyCode::Char(' ')), 3); // toggle it
        assert!(
            app.multi_selected.contains(&2),
            "toggle must hit gamma's original index, got {:?}",
            app.multi_selected
        );

        handle_form_multi_key(&mut app, key(KeyCode::Enter), 3);
        assert_eq!(app.mode, Mode::IssueForm);
        assert!(app.issue_form.unwrap().labels.contains(&2));
    }

    #[test]
    fn select_picker_enter_noop_on_no_matches_but_closes_when_empty() {
        let mut app = picker_test_app();
        app.mode = Mode::SelectField(1);
        handle_select_field_key(&mut app, key(KeyCode::Char('z')), 1); // no matches
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(
            app.mode,
            Mode::SelectField(1),
            "Enter must not pick from nothing"
        );

        app.start_picker(Vec::new(), 0); // truly empty picker
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn select_picker_enter_applies_filtered_pick() {
        let mut app = picker_test_app();
        app.mode = Mode::SelectField(1); // repo filter field
        handle_select_field_key(&mut app, key(KeyCode::Char('b')), 1);
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(app.filters.repo, "beta");
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_space_toggles_and_enter_applies() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.start_picker(vec!["low".into(), "high".into(), "urgent".into()], 0);
        app.mode = Mode::SelectFieldMulti(4);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // low
        handle_select_field_multi_key(&mut app, key(KeyCode::Down), 4);
        handle_select_field_multi_key(&mut app, key(KeyCode::Down), 4);
        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // urgent
        handle_select_field_multi_key(&mut app, key(KeyCode::Enter), 4);

        assert_eq!(app.filters.priority, vec!["low", "urgent"]);
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_empty_selection_clears_filter() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.filters.status = vec!["blocked".into()];
        app.start_picker(vec!["blocked".into(), "in-progress".into()], 0);
        app.multi_selected = [0].into_iter().collect();
        app.mode = Mode::SelectFieldMulti(5);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 5); // untoggle blocked
        handle_select_field_multi_key(&mut app, key(KeyCode::Enter), 5);

        assert!(app.filters.status.is_empty());
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_esc_discards_toggles() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.filters.priority = vec!["high".into()];
        app.start_picker(vec!["low".into(), "high".into()], 0);
        app.multi_selected = [1].into_iter().collect();
        app.mode = Mode::SelectFieldMulti(4);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // toggle low on
        handle_select_field_multi_key(&mut app, key(KeyCode::Esc), 4);

        assert_eq!(app.filters.priority, vec!["high"]);
        assert_eq!(app.mode, Mode::FilterMenu);
    }
}
