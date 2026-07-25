use super::list::input_prompt;
use super::prelude::*;
use super::widgets::centered;
use crate::tui::app::{
    ConfirmChoice, FILTER_FIELDS, INPUT_POPUP_WIDTH, ISSUE_FORM_FIELDS, InputKind,
    input_popup_width, input_scroll_skip,
};
use chrono::{Datelike, NaiveDate};

/// Single-line input popup used for search/filters/assignees/labels/title/
/// org/new-issue-title. Horizontally scrolls so the cursor always stays
/// visible when the value is wider than the box.
/// Centered close/reopen confirmation popup (`Mode::ConfirmState`), with a
/// `[ Yes ]  [ No ]` button row reusing the reversed-video focused-button
/// style from the inline comment editor.
pub(super) fn draw_confirm_popup(f: &mut Frame, app: &App, t: &Theme) {
    let action = app
        .selected_issue()
        .map(|i| match i.state {
            crate::provider::types::IssueState::Open => "close",
            crate::provider::types::IssueState::Closed => "reopen",
        })
        .unwrap_or("toggle");
    let message = match app.selected_issue() {
        Some(i) => format!("{action} issue #{}?", i.number),
        None => format!("{action} this issue?"),
    };

    let width = (message.len() as u16 + 4).max(24);
    let area = centered(f.area(), width, 4);
    f.render_widget(Clear, area);

    const YES: &str = "[ Yes ]";
    const NO: &str = "[ No ]";
    let gap = "  ";
    let inner_width = area.width.saturating_sub(2) as usize;
    let total = YES.len() + gap.len() + NO.len();
    let pad = " ".repeat(inner_width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    let buttons = Line::from(vec![
        Span::raw(pad),
        Span::styled(YES, button_style(app.confirm_choice == ConfirmChoice::Yes)),
        Span::raw(gap),
        Span::styled(NO, button_style(app.confirm_choice == ConfirmChoice::No)),
    ]);

    let para = Paragraph::new(vec![
        Line::styled(message, Style::default().fg(t.warning)),
        buttons,
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {action} issue ")),
    );
    f.render_widget(para, area);
}

pub(super) fn draw_filter_menu(f: &mut Frame, app: &App, t: &Theme) {
    let area = centered(f.area(), 60, FILTER_FIELDS.len() as u16 + 4);
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = FILTER_FIELDS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let value = app.current_filter_value(i);
            let style = if i == app.filter_menu_idx {
                Style::default().bg(t.selected_bg)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<28}"), style.fg(t.accent)),
                Span::styled(value, style),
            ]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" filters (Enter edit · c clear all · Esc close) "),
    );
    f.render_widget(list, area);
}

/// Rows for an option picker under the active type-ahead filter: a `/`
/// row while a filter is typed, then the filtered options. The highlight
/// is positional within the filtered view; multi-select `[x]` marks and
/// the "—" clear row key off original option indices. ASCII prefix on
/// purpose — emoji cell widths are unreliable across terminals.
pub(super) fn picker_items(
    app: &App,
    t: &Theme,
    multi: bool,
    clear_label: &str,
) -> Vec<ListItem<'static>> {
    let mut items: Vec<ListItem> = Vec::new();
    if !app.select_filter.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(" / ", Style::default().fg(t.accent)),
            Span::raw(app.select_filter.clone()),
            Span::styled("█", Style::default().fg(t.accent)),
        ])));
    }
    let filtered = app.filtered_select();
    if filtered.is_empty() {
        let msg = if app.select_options.is_empty() {
            " nothing available"
        } else {
            " no matches"
        };
        items.push(ListItem::new(Line::styled(
            msg.to_string(),
            Style::default().fg(t.dim),
        )));
        return items;
    }
    for (pos, (orig, opt)) in filtered.into_iter().enumerate() {
        let style = if pos == app.select_idx {
            Style::default().bg(t.selected_bg)
        } else {
            Style::default()
        };
        let text = if multi {
            let mark = if app.multi_selected.contains(&orig) {
                "[x]"
            } else {
                "[ ]"
            };
            format!(" {mark} {opt}")
        } else if opt == "\u{2014}" {
            format!(" \u{2014} {clear_label} \u{2014}")
        } else {
            format!(" {opt}")
        };
        items.push(ListItem::new(Line::from(Span::styled(text, style))));
    }
    items
}

/// Popup height for `rows` list items (+2 borders), clamped to the frame.
pub(super) fn picker_height(f: &Frame, rows: usize) -> u16 {
    (rows.max(1) as u16 + 2).min(f.area().height)
}

/// Default popup width for every picker but the PR one, which needs more
/// room for `owner/repo#number` entries.
pub(super) const PICKER_WIDTH: u16 = 50;
pub(super) const PR_PICKER_WIDTH: u16 = 60;

/// What distinguishes one picker popup from another. Everything else — the
/// item list, the centring, the border — is identical across all of them.
pub(super) struct PickerSpec {
    /// Full border title, including its surrounding spaces. Longer than the
    /// popup is wide in most cases; the border clips it.
    title: String,
    width: u16,
    /// Space toggles and items carry `[ ]`/`[x]` marks.
    multi: bool,
    /// Wording for the leading "clear this field" entry.
    clear_label: &'static str,
}

impl PickerSpec {
    /// A standard-width picker clearing to "clear".
    pub(super) fn new(title: impl Into<String>, multi: bool) -> Self {
        Self {
            title: title.into(),
            width: PICKER_WIDTH,
            multi,
            clear_label: "clear",
        }
    }

    /// Picker for a filter field (`Mode::SelectField` / `SelectFieldMulti`).
    pub(super) fn filter_field(idx: usize, multi: bool) -> Self {
        let field_name = FILTER_FIELDS[idx];
        let hint = if multi {
            "type filters · Space toggles · Enter accepts"
        } else {
            "type to filter · Enter picks · Esc cancels"
        };
        Self::new(format!(" select {field_name} ({hint}) "), multi)
    }

    /// Picker for a new-issue form field (`Mode::IssueFormSelect` /
    /// `IssueFormMulti`). These clear to "none" rather than "clear".
    pub(super) fn form_field(idx: usize, multi: bool) -> Self {
        let field_name = ISSUE_FORM_FIELDS[idx];
        let hint = if multi {
            "type filters · Space toggles · Enter accepts"
        } else {
            "type filters · Enter picks · Esc cancels"
        };
        Self {
            clear_label: "none",
            ..Self::new(format!(" {field_name} ({hint}) "), multi)
        }
    }

    /// Setting the selected issue's priority (`Mode::PrioritySet`).
    pub(super) fn priority() -> Self {
        Self::new(
            " set priority (type to filter · Enter sets · Esc cancels) ",
            false,
        )
    }

    /// Editing the selected issue's labels (`Mode::LabelsSet`).
    pub(super) fn labels() -> Self {
        Self::new(
            " set labels (type to filter · Space toggles · Enter accepts · Esc cancels) ",
            true,
        )
    }

    /// Choosing among several linked PRs (`Mode::PrPicker`).
    pub(super) fn pr_links() -> Self {
        Self {
            width: PR_PICKER_WIDTH,
            ..Self::new(
                " linked PRs (type to filter · Enter picks · Esc cancels) ",
                false,
            )
        }
    }
}

/// The one picker popup. Every `Mode` that shows a list of choices renders
/// through here; only the [`PickerSpec`] differs.
pub(super) fn draw_picker(f: &mut Frame, app: &App, t: &Theme, spec: PickerSpec) {
    let items = picker_items(app, t, spec.multi, spec.clear_label);
    let area = centered(f.area(), spec.width, picker_height(f, items.len()));
    f.render_widget(Clear, area);
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(spec.title));
    f.render_widget(list, area);
}

/// Single-line input popup used for search/filters/assignees/labels/title/
/// org/new-issue-title. Horizontally scrolls so the cursor always stays
/// visible when the value is wider than the box.
pub(super) fn draw_input_popup(f: &mut Frame, app: &App, t: &Theme, kind: InputKind) {
    let area = centered(f.area(), INPUT_POPUP_WIDTH, 3.min(f.area().height));
    f.render_widget(Clear, area);
    let width = input_popup_width(f.area().width) as usize;

    let skip = input_scroll_skip(app.input.cursor, width);
    let visible: String = app.input.buffer.chars().skip(skip).take(width).collect();
    let col = app.input.cursor - skip;

    let para = Paragraph::new(Line::from(cursor_spans(&visible, col))).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(" {} ", input_prompt(kind))),
    );
    f.render_widget(para, area);
}

/// The text with the char at `cursor` drawn as a block cursor (reversed
/// video); a reversed space when the cursor sits past the end of the text.
pub(super) fn cursor_spans(text: &str, cursor: usize) -> Vec<Span<'static>> {
    let byte = text
        .char_indices()
        .nth(cursor)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    let mut rest = text[byte..].chars();
    let under = rest.next().unwrap_or(' ').to_string();
    let after: String = rest.collect();
    vec![
        Span::raw(text[..byte].to_string()),
        Span::styled(under, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]
}

pub(super) fn draw_calendar_popup(f: &mut Frame, app: &App, t: &Theme, idx: usize) {
    let field_name = FILTER_FIELDS[idx];
    let cursor = app.calendar_cursor;

    let first = NaiveDate::from_ymd_opt(cursor.year(), cursor.month(), 1).unwrap();
    let next_first = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
    };
    let last = next_first.pred_opt().unwrap_or(next_first);
    let dow_offset = first.weekday().num_days_from_monday() as usize;
    let days_in_month = last.day();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("{} {}", cursor.format("%B"), cursor.year()),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(" Mo Tu We Th Fr Sa Su".to_string()));

    let mut day = 1u32;
    for _row in 0..6 {
        if day > days_in_month {
            break;
        }
        let mut week: Vec<Span> = Vec::new();
        for col in 0..7 {
            if day == 1 && col < dow_offset {
                week.push(Span::raw("   ".to_string()));
            } else if day <= days_in_month {
                let style = if day == cursor.day() {
                    Style::default()
                        .bg(t.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                week.push(Span::styled(format!("{:>2} ", day), style));
                day += 1;
            }
        }
        if !week.is_empty() {
            lines.push(Line::from(week));
        }
    }

    lines.push(Line::raw("".to_string()));
    lines.push(Line::from(vec![
        Span::styled("\u{2190}\u{2192} day  ", Style::default().fg(t.dim)),
        Span::styled("\u{2191}\u{2193} week  ", Style::default().fg(t.dim)),
        Span::styled("PgUp/PgDn month  ", Style::default().fg(t.dim)),
        Span::styled("Enter select  Esc cancel", Style::default().fg(t.dim)),
    ]));

    let height = lines.len() as u16 + 2;
    let area = centered(f.area(), 32, height);
    f.render_widget(Clear, area);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {field_name} ")),
    );
    f.render_widget(para, area);
}

pub(super) fn draw_help(f: &mut Frame, t: &Theme) {
    const HELP: &[(&str, &str)] = &[
        ("j/k ↑/↓", "move list / scroll detail region"),
        ("Space", "collapse/expand repo group"),
        ("←", "collapse repo group / back to list"),
        ("→", "expand repo group / into detail pane"),
        ("[ / ]", "collapse all / expand all"),
        ("Enter", "open issue in detail pane"),
        (
            "Tab",
            "next comment in pane / switch pane (Shift+Tab reverse)",
        ),
        ("Esc / q", "close detail pane"),
        ("o / O", "open issue / repo in browser"),
        ("y", "copy issue ref to clipboard"),
        ("/", "text search"),
        ("#", "jump to issue number"),
        ("f", "cycle state filter (open/closed/all)"),
        ("F", "filter editor (pickers + calendar)"),
        ("s / S", "cycle sort key / toggle direction"),
        ("w", "switch org/owner"),
        ("c", "add comment (inline, Tab to buttons, Ctrl+S save)"),
        ("e", "edit description / comment (detail pane)"),
        ("x", "close / reopen issue"),
        ("a", "edit assignees"),
        ("l", "edit labels"),
        ("t", "edit title"),
        ("p", "set priority"),
        ("P", "summarise linked PR (in detail pane)"),
        ("n", "new issue"),
        ("r", "reload"),
        ("q", "back / quit"),
    ];
    let area = centered(f.area(), 52, HELP.len() as u16 + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), Style::default().fg(t.accent).bold()),
                Span::raw(*v),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" keys ")
                .title_bottom(
                    Line::from(format!(" v{} ", env!("CARGO_PKG_VERSION"))).right_aligned(),
                ),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::provider::types::IssueState;
    use crate::tui::app::IssueForm;
    use crate::tui::app::Mode;

    #[test]
    fn confirm_popup_prompts_close_for_open_issue() {
        let app = confirm_app(IssueState::Open);
        let text = rendered_confirm_popup(&app);
        assert!(text.contains("close issue"), "got: {text}");
        assert!(text.contains("#114"), "got: {text}");
        assert!(text.contains("Yes"), "got: {text}");
        assert!(text.contains("No"), "got: {text}");
    }

    #[test]
    fn confirm_popup_prompts_reopen_for_closed_issue() {
        let app = confirm_app(IssueState::Closed);
        let text = rendered_confirm_popup(&app);
        assert!(text.contains("reopen issue"), "got: {text}");
    }

    #[test]
    fn confirm_popup_highlights_focused_button() {
        let mut app = confirm_app(IssueState::Open);

        app.confirm_choice = super::ConfirmChoice::Yes;
        let buf = render_confirm_buffer(&app);
        assert!(is_reversed_at(&buf, "Yes"));
        assert!(!is_reversed_at(&buf, "No"));

        app.confirm_choice = super::ConfirmChoice::No;
        let buf = render_confirm_buffer(&app);
        assert!(!is_reversed_at(&buf, "Yes"));
        assert!(is_reversed_at(&buf, "No"));
    }

    #[test]
    fn help_popup_renders_version() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_help(f, &Theme::default())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains(concat!("v", env!("CARGO_PKG_VERSION"))),
            "version not found in help popup"
        );
    }

    // ----------------------------------------------------------------------
    // Characterisation goldens (issue #87).
    //
    // These pin what the screen actually looks like today, so the refactor
    // phases that follow have something to preserve. They read geometry back
    // out of the rendered buffer rather than recomputing it from the same
    // constants the renderer used — a test that repeats the production
    // arithmetic proves only that the arithmetic was copied correctly.
    //
    // Do not edit these to make a refactor pass. A golden that needs to
    // change means behaviour changed; investigate that first.

    #[test]
    fn golden_filter_select_popup() {
        let app = picker_app(Mode::SelectField(1));
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "filter select popup width");
        let text = popup.text();
        // Field name comes from FILTER_FIELDS[1].
        assert!(
            text.contains("select repo (type to filter · Enter picks"),
            "title missing: {text}"
        );
        // Filter pickers label the clear entry "clear".
        assert!(text.contains("— clear —"), "clear entry missing: {text}");
        assert!(text.contains("alpha"), "options missing: {text}");
        // Single-select draws no checkbox marks.
        assert!(!text.contains("[ ]"), "unexpected multi marks: {text}");
    }

    #[test]
    fn golden_filter_select_multi_popup() {
        let mut app = picker_app(Mode::SelectFieldMulti(4));
        app.multi_selected.insert(2); // "beta"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "filter multi popup width");
        let text = popup.text();
        assert!(
            text.contains("select priority (type filters · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] beta"), "checked mark missing: {text}");
        assert!(text.contains("[ ] alpha"), "unchecked mark missing: {text}");
    }

    #[test]
    fn golden_priority_set_popup() {
        let app = picker_app(Mode::PrioritySet);
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "priority popup width");
        let text = popup.text();
        assert!(
            text.contains("set priority (type to filter · Enter sets"),
            "title missing: {text}"
        );
        assert!(!text.contains("[ ]"), "unexpected multi marks: {text}");
    }

    #[test]
    fn golden_labels_set_popup() {
        let mut app = picker_app(Mode::LabelsSet);
        app.multi_selected.insert(1); // "alpha"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "labels popup width");
        let text = popup.text();
        assert!(
            text.contains("set labels (type to filter · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] alpha"), "checked mark missing: {text}");
    }

    #[test]
    fn golden_pr_picker_popup() {
        let app = picker_app(Mode::PrPicker);
        let popup = popup_box(&render_app(&app, 100, 30));

        // The PR picker is the one wider popup.
        assert_eq!(popup.width, 60, "PR picker popup width");
        let text = popup.text();
        assert!(
            text.contains("linked PRs (type to filter · Enter picks · Esc cancels)"),
            "title missing: {text}"
        );
    }

    #[test]
    fn golden_issue_form_select_popup() {
        let mut app = picker_app(Mode::IssueFormSelect(4));
        app.issue_form = Some(IssueForm::new("repo".into()));
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "form select popup width");
        let text = popup.text();
        // Field name comes from ISSUE_FORM_FIELDS[4], with no "select" prefix.
        assert!(text.contains(" type ("), "title missing: {text}");
        assert!(
            text.contains("type filters · Enter picks · Esc cancels"),
            "single-select hint missing: {text}"
        );
        // Form pickers label the clear entry "none", not "clear".
        assert!(text.contains("— none —"), "none entry missing: {text}");
        assert!(!text.contains("— clear —"), "wrong clear label: {text}");
    }

    #[test]
    fn golden_issue_form_multi_popup() {
        let mut app = picker_app(Mode::IssueFormMulti(3));
        app.issue_form = Some(IssueForm::new("repo".into()));
        app.multi_selected.insert(3); // "gamma"
        let popup = popup_box(&render_app(&app, 100, 30));

        assert_eq!(popup.width, 50, "form multi popup width");
        let text = popup.text();
        assert!(
            text.contains(" labels (type filters · Space toggles"),
            "title missing: {text}"
        );
        assert!(text.contains("[x] gamma"), "checked mark missing: {text}");
    }

    /// Pickers are centred on the frame and sized to their content: one row
    /// per option plus the two border rows.
    #[test]
    fn golden_picker_is_centred_and_sized_to_content() {
        let popup = popup_box(&render_app(&picker_app(Mode::SelectField(1)), 100, 30));
        // 4 options + 2 borders.
        assert_eq!(popup.height, 6, "picker height");
        assert_eq!(popup.width, 50, "picker width");
        assert_eq!(popup.x, (100 - popup.width) / 2, "picker not centred in x");
        assert_eq!(popup.y, (30 - popup.height) / 2, "picker not centred in y");
    }

    /// Picker titles are longer than the popups are wide, so the border
    /// clips them. That clipping is the current appearance and any change to
    /// popup width or title wording moves it.
    #[test]
    fn golden_picker_titles_clip_at_the_border() {
        let cases = [
            (
                Mode::SelectField(1),
                "┌ select repo (type to filter · Enter picks · Esc┐",
            ),
            (
                Mode::PrioritySet,
                "┌ set priority (type to filter · Enter sets · Esc┐",
            ),
        ];
        for (mode, want) in cases {
            let popup = popup_box(&render_app(&picker_app(mode), 100, 30));
            assert_eq!(popup.rows[0], want, "clipped title for {mode:?}");
        }

        // The PR picker is wide enough that its title is not clipped.
        let popup = popup_box(&render_app(&picker_app(Mode::PrPicker), 100, 30));
        assert!(
            popup.rows[0].contains("Esc cancels) "),
            "PR picker title unexpectedly clipped: {}",
            popup.rows[0]
        );
    }

    #[test]
    fn golden_picker_type_ahead_filter_row() {
        let mut app = picker_app(Mode::SelectField(1));
        app.picker_filter_push('a');
        app.picker_filter_push('l');
        let popup = popup_box(&render_app(&app, 100, 30));

        let text = popup.text();
        assert!(text.contains("/ al"), "filter row missing: {text}");
        assert!(text.contains("alpha"), "match missing: {text}");
        assert!(!text.contains("beta"), "non-match still shown: {text}");
    }

    #[test]
    fn golden_picker_reports_empty_and_no_match_distinctly() {
        let mut empty = test_app();
        empty.start_picker(vec![], 0);
        empty.mode = Mode::SelectField(1);
        assert!(
            popup_box(&render_app(&empty, 100, 30))
                .text()
                .contains("nothing available")
        );

        let mut no_match = picker_app(Mode::SelectField(1));
        no_match.picker_filter_push('z');
        assert!(
            popup_box(&render_app(&no_match, 100, 30))
                .text()
                .contains("no matches")
        );
    }

    // ---- PR summary row model -------------------------------------------
}
