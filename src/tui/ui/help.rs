//! The help viewer (#184): a topic strip, a live-status block, and one page of
//! text, scrollable.
//!
//! The row count is computed by [`help_lines`] for the renderer *and* for the
//! key handler's scroll clamp (`help_max_scroll`), so a clamp measured off the
//! terminal cannot drift from the rows on screen — the same rule as the PR
//! summary popup.

use super::popups::{LIST_HELP, draw_session_help, key_table_lines};
use super::prelude::*;
use super::widgets::{centered, render_region_scrollbar};
use crate::tui::app::{HelpTopic, StatusLine, Tone};
use crate::tui::markdown;

const HELP_WIDTH: u16 = 80;

/// The popup's outer height for a frame `frame_height` tall, before `centered`
/// clamps it to the frame.
fn help_outer_height(frame_height: u16) -> u16 {
    (frame_height * 4 / 5).max(12)
}

/// Inner text width: the popup less its borders and a column kept clear for the
/// scrollbar, so the thumb never draws over the last character.
pub fn help_inner_width(frame_width: u16) -> u16 {
    HELP_WIDTH.min(frame_width).saturating_sub(3)
}

/// Inner text height for a frame `frame_height` tall. Shared with the key
/// handler so `PageUp`/`PageDown` step one viewport.
pub fn help_inner_height(frame_height: u16) -> u16 {
    help_outer_height(frame_height)
        .min(frame_height)
        .saturating_sub(2)
}

fn help_area(frame: Rect) -> Rect {
    centered(frame, HELP_WIDTH, help_outer_height(frame.height))
}

fn status_lines(rows: &[StatusLine], t: &Theme) -> Vec<Line<'static>> {
    let width = rows.iter().map(|r| r.label.len()).max().unwrap_or(0);
    rows.iter()
        .map(|r| {
            let tone = match r.tone {
                Tone::On => t.open,
                Tone::Off => t.dim,
                Tone::Failed => t.error,
            };
            Line::from(vec![
                Span::styled(
                    format!(" {:<width$}  ", r.label),
                    Style::default().fg(t.dim),
                ),
                Span::styled(r.value.clone(), Style::default().fg(tone)),
            ])
        })
        .collect()
}

/// Every wrapped row of `topic`'s page: the live-status block, then the text.
pub fn help_lines(app: &App, topic: HelpTopic, width: u16, t: &Theme) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut lines = status_lines(&app.help_status(topic), t);
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    match topic.markdown() {
        Some(md) => lines.extend(markdown::render_with_links(md, width, t).0),
        None => {
            for note in [
                " F1 opens help for where you are, even while typing.",
                " \u{2190}/\u{2192} or Tab switch page \u{b7} j/k scroll \u{b7} Esc closes.",
            ] {
                lines.push(Line::from(Span::styled(
                    note,
                    Style::default().fg(t.dim).italic(),
                )));
            }
            lines.push(Line::default());
            lines.extend(key_table_lines(LIST_HELP, t));
        }
    }
    linkmap::wrap(&lines, &[], width).0
}

/// The furthest the page can usefully scroll at a `cols` × `rows` terminal.
pub fn help_max_scroll(app: &App, topic: HelpTopic, cols: u16, rows: u16) -> u16 {
    let content = help_lines(app, topic, help_inner_width(cols), &Theme::default()).len();
    u16::try_from(content)
        .unwrap_or(u16::MAX)
        .saturating_sub(help_inner_height(rows))
}

/// The strip across the top: every page, the open one highlighted.
fn topic_strip(open: HelpTopic, t: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(" help ", Style::default().fg(t.dim))];
    for topic in HelpTopic::ALL {
        let style = if topic == open {
            Style::default().fg(t.accent).bold().reversed()
        } else {
            Style::default().fg(t.dim)
        };
        spans.push(Span::styled(format!(" {} ", topic.title()), style));
    }
    Line::from(spans)
}

pub(super) fn draw_help(f: &mut Frame, app: &App, t: &Theme, topic: HelpTopic) {
    // `F12 ?` over a session keeps its own short table: a session forwards
    // almost every key to the agent, so the app's pages would mislead there.
    if app.help_is_session() {
        draw_session_help(f, t);
        return;
    }
    let area = help_area(f.area());
    f.render_widget(Clear, area);

    let lines = help_lines(app, topic, help_inner_width(f.area().width), t);
    let content_h = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let viewport_h = area.height.saturating_sub(2);
    // Clamp here as well as in the key handler: a resize can shrink the
    // viewport after the scroll was set.
    let scroll = app.help.scroll.min(content_h.saturating_sub(viewport_h));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(topic_strip(topic, t))
        .title_bottom(Line::from(Span::styled(
            " \u{2190}/\u{2192} page \u{b7} j/k scroll \u{b7} Esc close ",
            Style::default().fg(t.dim),
        )))
        .title_bottom(Line::from(format!(" v{} ", env!("CARGO_PKG_VERSION"))).right_aligned());
    // Wrapping is already applied by `help_lines`, so the Paragraph must not
    // wrap again — otherwise a drawn row would stop matching its index.
    f.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
    render_region_scrollbar(f, t, area, content_h, viewport_h, scroll);
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::tui::app::Mode;
    use crate::typesafe::Status;

    fn text_of(buf: &ratatui::buffer::Buffer) -> String {
        let area = *buf.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn help_app(topic: HelpTopic) -> App {
        let mut app = test_app();
        app.mode = Mode::Help(topic);
        app
    }

    #[test]
    fn the_strip_lists_every_page_and_the_status_block_opens_a_feature_page() {
        let mut app = help_app(HelpTopic::Search);
        app.typesafe = Status::new(false, true, Some("k"));
        let text = text_of(&render_app(&app, 100, 30));
        for topic in HelpTopic::ALL {
            assert!(
                text.contains(topic.title()),
                "{} on the strip",
                topic.title()
            );
        }
        assert!(text.contains("semantic search"), "status label");
        assert!(text.contains("on"), "status value");
        assert!(text.contains("Turning it on"), "the page's own text");
    }

    #[test]
    fn the_typesafe_page_shows_the_whole_setup_and_never_a_key() {
        let mut app = help_app(HelpTopic::TypeSafe);
        app.typesafe = Status::new(true, false, Some("sk-super-secret-value"));
        let text = text_of(&render_app(&app, 100, 40));
        for want in [
            "TYPESAFE_API_KEY",
            "set",
            "send_issue_text",
            "infer_priority_ranks",
            "priority ranks",
        ] {
            assert!(text.contains(want), "{want}");
        }
        assert!(!text.contains("sk-super-secret-value"));
    }

    /// The keys page is the app's own table, so a key added to it appears in
    /// help without anyone editing prose.
    #[test]
    fn the_keys_page_holds_every_key_in_the_table() {
        let app = help_app(HelpTopic::Keys);
        let text = text_of(&render_app(&app, 100, 80));
        for (key, what) in LIST_HELP.iter().filter(|(k, _)| !k.is_empty()) {
            assert!(text.contains(key.trim()), "{key}");
            assert!(text.contains(what), "{what}");
        }
        assert!(
            LIST_HELP.iter().any(|(k, _)| k.contains("F1")),
            "the key table itself documents F1"
        );
    }

    /// The clamp the key handler uses and the rows the renderer draws come from
    /// one function, so scrolling to the maximum must bring the last row into
    /// view — for every page, at a small terminal where every page overflows.
    #[test]
    fn scrolling_to_the_clamp_shows_the_last_row_of_every_page() {
        let (cols, rows) = (80, 24);
        for topic in HelpTopic::ALL {
            let mut app = help_app(topic);
            app.typesafe = Status::new(true, true, Some("k"));
            let lines = help_lines(&app, topic, help_inner_width(cols), &Theme::default());
            let last = lines
                .iter()
                .rev()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .find(|t| !t.trim().is_empty())
                .expect("a non-empty page");

            let max = help_max_scroll(&app, topic, cols, rows);
            assert!(max > 0, "{topic:?} should overflow 80x24");
            // Exactly the overflow: one more and the handler would scroll past
            // the last row into blank space.
            assert_eq!(
                max as usize + help_inner_height(rows) as usize,
                lines.len(),
                "{topic:?}"
            );

            app.help.scroll = 0;
            assert!(
                !text_of(&render_app(&app, cols, rows)).contains(last.trim()),
                "{topic:?}: the last row is off-screen at the top"
            );
            app.help.scroll = max;
            assert!(
                text_of(&render_app(&app, cols, rows)).contains(last.trim()),
                "{topic:?}: the last row {last:?} must be visible at the clamp"
            );
        }
    }

    /// A resize after scrolling must not leave the viewer past the end.
    #[test]
    fn a_stale_scroll_is_clamped_when_drawn() {
        let mut app = help_app(HelpTopic::Search);
        app.help.scroll = u16::MAX;
        let text = text_of(&render_app(&app, 80, 24));
        assert!(!text.trim().is_empty());
        assert!(
            text.contains("Only `/`") || text.contains("Only /"),
            "the final bullet, not a blank popup: {text}"
        );
    }

    #[test]
    fn every_page_renders_at_awkward_sizes_without_panicking() {
        for topic in HelpTopic::ALL {
            for (w, h) in [(80, 24), (40, 12), (20, 6), (1, 1), (200, 60)] {
                let mut app = help_app(topic);
                app.help.scroll = 3;
                let _ = render_app(&app, w, h);
            }
        }
    }

    #[test]
    fn help_over_a_session_still_draws_the_short_session_table() {
        let mut app = help_app(HelpTopic::Keys);
        app.open_session_help();
        let text = text_of(&render_app(&app, 80, 30));
        assert!(text.contains("session keys"));
        assert!(text.contains("F12 d"));
        assert!(!text.contains("semantic"), "no topic strip over a session");
    }

    #[test]
    fn the_input_popup_says_where_help_is() {
        let mut app = test_app();
        app.mode = Mode::Input(crate::tui::app::InputKind::Search);
        assert!(text_of(&render_app(&app, 80, 24)).contains("F1 help"));
    }

    // ---- the info-bar indicator ----

    fn indicator_app() -> App {
        let mut app = confirm_app(crate::provider::types::IssueState::Open);
        app.mode = Mode::Normal;
        app.typesafe = Status::new(false, true, Some("k"));
        app.set_text_filter("disk trouble".into());
        app
    }

    #[test]
    fn the_info_bar_follows_a_search_and_names_a_failure() {
        let mut app = indicator_app();
        let bar = |app: &App| text_of(&render_app(app, 120, 24));
        assert!(!bar(&app).contains("semantic:"), "nothing asked yet");

        let (g, _, _) = app.begin_semantic_search().unwrap();
        assert!(bar(&app).contains("semantic: searching…"));

        app.apply_semantic_search(g, Ok(std::collections::HashSet::new()));
        assert!(bar(&app).contains("semantic: +0"));

        app.set_text_filter("another".into());
        let (g, _, _) = app.begin_semantic_search().unwrap();
        app.apply_semantic_search(g, Err("boom".into()));
        assert!(bar(&app).contains("semantic: off (boom)"));
    }

    #[test]
    fn the_info_bar_is_unchanged_without_the_consent() {
        let mut app = indicator_app();
        app.typesafe = Status::default();
        let (g, _, _) = app.begin_semantic_search().unwrap();
        app.apply_semantic_search(g, Ok(std::collections::HashSet::new()));
        assert!(!text_of(&render_app(&app, 120, 24)).contains("semantic:"));
    }
}
