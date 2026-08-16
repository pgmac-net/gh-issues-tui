//! Drawing a harness session (#23).
//!
//! The child's screen comes from its `vt100` parser, rendered by `tui-term`'s
//! `PseudoTerminal`. Full-frame, less one status row: an agent's own TUI
//! needs the space, and it keeps the PTY size a plain function of the
//! terminal size.

use super::prelude::*;
use crate::tui::app::SessionStatus;
use crate::tui::harness::HarnessRegistry;
use tui_term::widget::PseudoTerminal;

/// Draw the active session. Falls back to a message rather than a blank
/// screen if the registry has no PTY for it — that can only happen if a
/// session was dismissed between the keypress and this draw.
pub(super) fn draw_harness(
    f: &mut Frame,
    app: &App,
    t: &Theme,
    registry: &HarnessRegistry,
    area: Rect,
) {
    let areas = layout::harness_areas(area);
    let Some(session) = app.harness.active_meta() else {
        return;
    };

    match registry.parser(session.id).and_then(|p| p.lock().ok()) {
        Some(parser) => {
            let screen = parser.screen();
            f.render_widget(PseudoTerminal::new(screen), areas.pane);
        }
        None => {
            let msg = Paragraph::new(Line::from(Span::styled(
                "session has no terminal (it was dismissed)",
                Style::default().fg(t.error),
            )));
            f.render_widget(msg, areas.pane);
        }
    }

    if areas.status.height > 0 {
        f.render_widget(status_line(session, t), areas.status);
    }
}

/// The one reserved row: which issue and harness this is, how it is doing,
/// and the chord that gets you out. The hint is always shown because `F12` is
/// the only key the TUI keeps and nothing else on screen advertises it.
fn status_line<'a>(session: &'a crate::tui::app::SessionMeta, t: &Theme) -> Paragraph<'a> {
    let (state, state_style) = match session.status {
        SessionStatus::Running => ("running".to_string(), Style::default().fg(t.open)),
        SessionStatus::Exited(code) => (
            format!("exited {code}"),
            Style::default().fg(if code == 0 { t.closed } else { t.error }),
        ),
    };
    let hint = if session.status.is_running() {
        "F12 d detach · s switch · k kill · n new · ? help"
    } else {
        "j/k scroll · x dismiss · q back · F12 s switch"
    };

    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", session.issue_ref),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}] ", session.harness),
            Style::default().fg(t.dim),
        ),
        Span::styled(state, state_style),
        Span::raw("  "),
        Span::styled(hint, Style::default().fg(t.dim)),
    ]))
}
