//! Keys for the help viewer (#184).

use super::shared::{help_page_rows, help_scroll_max};
use super::*;

pub(crate) fn handle_help_key(app: &mut App, key: KeyEvent) {
    // `F12 ?` over a session shows a short table with nothing to scroll, and
    // has always closed on any key.
    if app.help_is_session() {
        app.close_help();
        return;
    }
    match key.code {
        // `F1` toggles in `handle_key` before it reaches here.
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => app.close_help(),
        KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => app.help_switch(1),
        KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => app.help_switch(-1),
        KeyCode::Char('j') | KeyCode::Down => scroll(app, 1),
        KeyCode::Char('k') | KeyCode::Up => scroll(app, -1),
        KeyCode::PageDown => scroll(app, help_page_rows()),
        KeyCode::PageUp => scroll(app, -help_page_rows()),
        KeyCode::Home | KeyCode::Char('g') => app.help.scroll = 0,
        KeyCode::End | KeyCode::Char('G') => app.help.scroll = help_scroll_max(app),
        _ => {}
    }
}

fn scroll(app: &mut App, delta: i16) {
    let max = help_scroll_max(app);
    app.help.scroll_by(delta, max);
}

#[cfg(test)]
mod tests {
    use super::super::handle_key;
    use super::super::testutil::{app_with_issue, key, test_client};
    use super::*;
    use crate::config::builtin_harnesses;
    use crate::tui::app::HelpTopic;
    use crate::tui::harness::{HarnessRegistry, HarnessSettings};

    /// Everything `handle_key` takes besides the app and the key.
    struct Rig {
        registry: HarnessRegistry,
        settings: HarnessSettings,
        tx: mpsc::UnboundedSender<AppEvent>,
        _rx: mpsc::UnboundedReceiver<AppEvent>,
    }

    fn rig() -> Rig {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        Rig {
            registry: HarnessRegistry::default(),
            settings: HarnessSettings {
                default_harness: None,
                harnesses: builtin_harnesses(),
                workspace_roots: Vec::new(),
                cwd_repo: None,
                cwd: std::path::PathBuf::from("/"),
            },
            tx,
            _rx,
        }
    }

    fn press(app: &mut App, r: &mut Rig, code: KeyCode) {
        let mut hx = HarnessCtx {
            registry: &mut r.registry,
            settings: &r.settings,
            tx: &r.tx,
        };
        handle_key(app, key(code), &test_client(), &r.tx, &mut hx);
    }

    fn app() -> App {
        app_with_issue(&[]).0
    }

    /// `?` is a typed character in a text input, which is why `F1` exists.
    #[test]
    fn question_mark_types_in_a_search_but_f1_opens_help_over_it() {
        let mut app = app();
        let mut r = rig();
        press(&mut app, &mut r, KeyCode::Char('/'));
        press(&mut app, &mut r, KeyCode::Char('a'));
        press(&mut app, &mut r, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Input(InputKind::Search));
        assert_eq!(app.input.buffer, "a?", "typed, not help");

        press(&mut app, &mut r, KeyCode::F(1));
        assert_eq!(app.mode, Mode::Help(HelpTopic::Search));
        press(&mut app, &mut r, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Input(InputKind::Search));
        assert_eq!(app.input.buffer, "a?", "the half-typed search survives");
    }

    #[test]
    fn f1_and_question_mark_both_open_help_from_the_list_and_f1_closes_it() {
        let mut app = app();
        let mut r = rig();
        press(&mut app, &mut r, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Help(HelpTopic::Keys));
        press(&mut app, &mut r, KeyCode::F(1));
        assert_eq!(app.mode, Mode::Normal, "F1 toggles");
        press(&mut app, &mut r, KeyCode::F(1));
        assert_eq!(app.mode, Mode::Help(HelpTopic::Keys));
        press(&mut app, &mut r, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Normal, "? closes too");
    }

    /// F1 opens help from popups, not just the list.
    #[test]
    fn f1_works_from_a_popup() {
        let mut app = app();
        let mut r = rig();
        app.mode = Mode::ConfirmState;
        press(&mut app, &mut r, KeyCode::F(1));
        assert_eq!(app.mode, Mode::Help(HelpTopic::Keys));
        press(&mut app, &mut r, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::ConfirmState);
    }

    /// Inside a session every key is the agent's, F1 included.
    #[test]
    fn f1_is_left_to_the_agent_inside_a_session() {
        let mut app = app();
        let mut r = rig();
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.attach(id);
        app.mode = Mode::Harness;
        press(&mut app, &mut r, KeyCode::F(1));
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn f12_question_mark_opens_the_session_table_and_any_key_closes_it() {
        let mut app = app();
        let mut r = rig();
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.attach(id);
        app.mode = Mode::Harness;
        press(&mut app, &mut r, KeyCode::F(12));
        press(&mut app, &mut r, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Help(HelpTopic::Keys));
        assert!(app.help_is_session());

        press(&mut app, &mut r, KeyCode::Char('x'));
        assert_eq!(app.mode, Mode::Harness, "back in the session");
    }

    /// The change from before: help no longer closes on any key, so a page can
    /// be read and scrolled.
    #[test]
    fn an_unbound_key_does_not_close_the_viewer() {
        let mut app = app();
        let mut r = rig();
        press(&mut app, &mut r, KeyCode::Char('?'));
        press(&mut app, &mut r, KeyCode::Char('x'));
        press(&mut app, &mut r, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Help(HelpTopic::Keys));
    }

    #[test]
    fn arrows_tab_and_h_l_switch_pages() {
        let mut app = app();
        let mut r = rig();
        press(&mut app, &mut r, KeyCode::Char('?'));
        for (code, want) in [
            (KeyCode::Right, HelpTopic::Search),
            (KeyCode::Tab, HelpTopic::Readiness),
            (KeyCode::Char('l'), HelpTopic::Priority),
            (KeyCode::Left, HelpTopic::Readiness),
            (KeyCode::BackTab, HelpTopic::Search),
            (KeyCode::Char('h'), HelpTopic::Keys),
        ] {
            press(&mut app, &mut r, code);
            assert_eq!(app.mode, Mode::Help(want), "{code:?}");
        }
    }

    /// Scroll limits depend on the live terminal, so assert against the clamp
    /// the handler itself uses rather than a size this test cannot control.
    #[test]
    fn scroll_keys_stay_inside_the_page() {
        let mut app = app();
        let mut r = rig();
        press(&mut app, &mut r, KeyCode::Char('?'));
        let max = help_scroll_max(&app);

        press(&mut app, &mut r, KeyCode::End);
        assert_eq!(app.help.scroll, max);
        press(&mut app, &mut r, KeyCode::Char('j'));
        press(&mut app, &mut r, KeyCode::PageDown);
        assert_eq!(app.help.scroll, max, "cannot pass the end");

        press(&mut app, &mut r, KeyCode::Home);
        assert_eq!(app.help.scroll, 0);
        press(&mut app, &mut r, KeyCode::Char('k'));
        press(&mut app, &mut r, KeyCode::PageUp);
        assert_eq!(app.help.scroll, 0, "cannot pass the top");

        press(&mut app, &mut r, KeyCode::Down);
        assert_eq!(app.help.scroll, 1.min(max));
    }
}
