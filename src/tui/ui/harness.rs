//! Drawing a harness session (#23).
//!
//! The child's screen comes from its `vt100` parser, rendered by `tui-term`'s
//! `PseudoTerminal`. Full-frame, less two chrome rows: an identity row above
//! saying whose session this is (#132) and a key row below. An agent's own TUI
//! needs the rest of the space, and it keeps the PTY size a plain function of
//! the terminal size.

use super::prelude::*;
use crate::tui::app::SessionStatus;
use crate::tui::harness::HarnessRegistry;
use tui_term::widget::PseudoTerminal;

/// Shown reversed at the head of the identity row. A session is drawn
/// full-frame and the agent owns every pixel above the chrome, so without this
/// there is nothing on screen naming what launched the agent.
const BRAND: &str = " gh-issues-tui ";

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

    if areas.header.height > 0 {
        f.render_widget(identity_line(session, t, areas.header.width), areas.header);
    }
    if areas.status.height > 0 {
        f.render_widget(key_line(session, t), areas.status);
    }
}

/// The identity row: who launched this, which ticket, which harness, and
/// whether the child is still alive.
///
/// Fitted to `width` by dropping the least load-bearing part first. The brand
/// is never dropped — it is the reason the row exists.
fn identity_line<'a>(
    session: &'a crate::tui::app::SessionMeta,
    t: &Theme,
    width: u16,
) -> Paragraph<'a> {
    let (state, state_style) = match session.status {
        SessionStatus::Running => ("running", Style::default().fg(t.open)),
        SessionStatus::Exited(_) => ("exited", Style::default().fg(t.error)),
    };
    let dot = Span::styled("\u{25cf} ", state_style);

    let mut spans = vec![Span::styled(
        BRAND,
        Style::default()
            .fg(t.accent)
            .add_modifier(Modifier::REVERSED),
    )];

    // Everything after the brand, in the order it gets sacrificed: title
    // first, then the ref shortens. The tail (harness, dot, state) is the
    // cheapest and most load-bearing, so it always stays.
    let tail_width = 3 + session.harness.chars().count() + 2 + state.chars().count();
    let budget = (width as usize)
        .saturating_sub(BRAND.chars().count())
        .saturating_sub(tail_width);

    let issue_ref = fit_ref(&session.issue_ref, budget);
    spans.push(Span::styled(
        format!(" {issue_ref}"),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    ));

    let spent = issue_ref.chars().count() + 1;
    let for_title = budget.saturating_sub(spent);
    // A title needs its " · " lead-in plus something worth reading; below that
    // the ellipsis costs more than it conveys.
    if !session.title.is_empty() && for_title >= 6 {
        // Terminal default foreground: the title is the only prose on the row
        // and should read as plainly as the agent's own output.
        spans.push(Span::raw(format!(
            " \u{00b7} {}",
            truncate(&session.title, for_title - 3)
        )));
    }

    spans.push(Span::styled(
        format!(" \u{00b7} {} ", session.harness),
        Style::default().fg(t.dim),
    ));
    spans.push(dot);
    spans.push(Span::styled(state, state_style));

    Paragraph::new(Line::from(spans))
}

/// The key row. Which keys apply depends on whether the child is still there:
/// a live session forwards everything but the `F12` chord, while an exited one
/// is just a screen to read and dismiss.
fn key_line<'a>(session: &'a crate::tui::app::SessionMeta, t: &Theme) -> Paragraph<'a> {
    let hint = if session.status.is_running() {
        "F12 d detach \u{00b7} s switch \u{00b7} k kill \u{00b7} n new \u{00b7} F12 F12 literal \u{00b7} ? help"
    } else {
        "j/k scroll \u{00b7} x dismiss \u{00b7} q back \u{00b7} F12 s switch"
    };
    let mut spans = vec![Span::styled(format!(" {hint}"), Style::default().fg(t.dim))];
    if let SessionStatus::Exited(code) = session.status {
        spans.push(Span::styled(
            format!("   exit {code}"),
            Style::default().fg(if code == 0 { t.closed } else { t.error }),
        ));
    }
    Paragraph::new(Line::from(spans))
}

/// Shorten `owner/repo#number` to fit `budget`, dropping the owner before
/// touching the part that identifies the issue.
fn fit_ref(issue_ref: &str, budget: usize) -> String {
    if issue_ref.chars().count() <= budget {
        return issue_ref.to_string();
    }
    if let Some((_owner, rest)) = issue_ref.split_once('/') {
        let short = format!("\u{2026}/{rest}");
        if short.chars().count() <= budget {
            return short;
        }
    }
    truncate(issue_ref, budget)
}

/// Truncate to `max` columns, spending the last one on an ellipsis. Counts
/// chars rather than bytes so a multi-byte title cannot panic the slice.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::tui::app::Mode;

    /// An app showing a session for `issue_ref`/`title`, in `status`.
    fn session_app(issue_ref: &str, title: &str, status: SessionStatus) -> App {
        let mut app = test_app();
        let id = app.harness.register(
            issue_ref.to_string(),
            "claude".to_string(),
            title.to_string(),
        );
        if let SessionStatus::Exited(code) = status {
            app.harness.mark_exited(id, code);
        }
        app.harness.attach(id);
        app.mode = Mode::Harness;
        app
    }

    /// Row `y` of a rendered buffer, trailing blanks trimmed.
    fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let w = buf.area().width;
        (0..w)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn identity_row_names_the_launcher_the_issue_and_the_title() {
        let app = session_app(
            "pgmac-net/gh-issues-tui#132",
            "Send to Claude/harness",
            SessionStatus::Running,
        );
        let top = row(&render_app(&app, 120, 20), 0);
        assert!(top.contains("gh-issues-tui"), "got: {top}");
        assert!(top.contains("pgmac-net/gh-issues-tui#132"), "got: {top}");
        assert!(top.contains("Send to Claude/harness"), "got: {top}");
        assert!(top.contains("claude"), "got: {top}");
        assert!(top.contains("running"), "got: {top}");
    }

    #[test]
    fn the_brand_is_drawn_reversed_so_it_reads_as_a_badge() {
        let app = session_app("o/r#1", "t", SessionStatus::Running);
        let buf = render_app(&app, 120, 20);
        // First cell of the row is the badge's leading pad.
        assert!(
            buf[(0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "the brand must stand out from the agent's own output"
        );
    }

    #[test]
    fn the_key_row_documents_the_f12_chords_while_the_agent_is_alive() {
        let app = session_app("o/r#1", "t", SessionStatus::Running);
        let buf = render_app(&app, 120, 20);
        let keys = row(&buf, 19);
        assert!(keys.contains("F12 d detach"), "got: {keys}");
        assert!(keys.contains("F12 F12 literal"), "got: {keys}");
        assert!(keys.contains("? help"), "got: {keys}");
    }

    #[test]
    fn an_exited_session_swaps_the_keys_and_shows_its_code() {
        let app = session_app("o/r#1", "t", SessionStatus::Exited(3));
        let buf = render_app(&app, 120, 20);
        assert!(row(&buf, 0).contains("exited"), "top: {}", row(&buf, 0));
        let keys = row(&buf, 19);
        assert!(keys.contains("x dismiss"), "got: {keys}");
        assert!(keys.contains("exit 3"), "got: {keys}");
        assert!(
            !keys.contains("F12 d detach"),
            "nothing is listening: {keys}"
        );
    }

    #[test]
    fn the_brand_survives_a_narrow_terminal_even_as_the_title_goes() {
        let app = session_app(
            "pgmac-net/gh-issues-tui#132",
            "a title far too long to fit in this width",
            SessionStatus::Running,
        );
        let top = row(&render_app(&app, 56, 20), 0);
        assert!(
            top.contains("gh-issues-tui"),
            "brand must never elide: {top}"
        );
        assert!(top.contains("running"), "state must never elide: {top}");
        assert!(
            !top.contains("a title far too long"),
            "the title is what gives way first: {top}"
        );
    }

    #[test]
    fn fit_ref_drops_the_owner_before_the_issue_number() {
        // The number is the part that identifies the ticket; the owner is
        // recoverable from context, so it goes first.
        let got = fit_ref("pgmac-net/gh-issues-tui#132", 22);
        assert_eq!(got, "\u{2026}/gh-issues-tui#132");
    }

    #[test]
    fn fit_ref_leaves_a_ref_that_already_fits_alone() {
        assert_eq!(fit_ref("o/r#1", 40), "o/r#1");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // A byte slice here would panic mid-codepoint.
        assert_eq!(truncate("ααααα", 3), "αα\u{2026}");
    }

    #[test]
    fn an_empty_title_leaves_no_dangling_separator() {
        let app = session_app("o/r#1", "", SessionStatus::Running);
        let top = row(&render_app(&app, 120, 20), 0);
        assert!(
            !top.contains("#1 \u{00b7} \u{00b7}"),
            "an issue with no title must not draw an empty slot: {top}"
        );
    }

    #[test]
    fn a_two_row_terminal_keeps_the_keys_and_drops_the_identity_row() {
        // Pinned here as well as in `layout`: this is the size at which the
        // renderer must not index a zero-height region.
        let app = session_app("o/r#1", "t", SessionStatus::Running);
        let buf = render_app(&app, 80, 2);
        assert!(row(&buf, 1).contains("F12 d detach"));
        assert!(!row(&buf, 0).contains("gh-issues-tui"));
    }
}
