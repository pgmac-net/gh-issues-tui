use super::popups::cursor_spans;
use super::prelude::*;
use super::widgets::centered;
use crate::tui::app::input_scroll_skip;
use crate::tui::app::{
    CommentFocus, ISSUE_FORM_CANCEL_ROW, ISSUE_FORM_CREATE_ROW, ISSUE_FORM_DESC_HEIGHT,
    ISSUE_FORM_FIELDS, ISSUE_FORM_LABEL_WIDTH, ISSUE_FORM_WIDTH, IssueForm, cursor_row,
    issue_form_width, wrap_lines,
};

/// The single inline new-issue form: title and description edit in place,
/// choice fields show their current selection and open a picker popup on
/// Enter, `[ Create ]`/`[ Cancel ]` sit at the bottom. `Tab`/`Shift+Tab`
/// move focus; the focused row is highlighted (a cursor for text fields,
/// a background fill for choice fields and the buttons).
pub(super) fn draw_issue_form(f: &mut Frame, app: &App, t: &Theme) {
    let Some(form) = &app.issue_form else { return };
    let value_width =
        (issue_form_width(f.area().width) as usize).saturating_sub(ISSUE_FORM_LABEL_WIDTH);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(form_title_line(form, t, value_width));
    lines.extend(form_description_lines(form, t, value_width));
    for (i, name) in ISSUE_FORM_FIELDS.iter().enumerate().skip(2) {
        let focused = form.field_idx == i;
        let style = if focused {
            Style::default().bg(t.selected_bg)
        } else {
            Style::default()
        };
        let value = if form.options.is_none() {
            "loading…".to_string()
        } else {
            form.field_display(i)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {name:<ISSUE_FORM_LABEL_WIDTH$}"),
                style.fg(t.accent),
            ),
            Span::styled(value, style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(issue_form_button_line(
        form,
        t,
        value_width + ISSUE_FORM_LABEL_WIDTH,
    ));

    let area = centered(
        f.area(),
        ISSUE_FORM_WIDTH,
        (lines.len() as u16 + 2).min(f.area().height),
    );
    f.render_widget(Clear, area);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(
                " new issue in {} (Tab move · Enter edit/activate · Esc cancel) ",
                form.repo
            )),
    );
    f.render_widget(para, area);
}

/// The title row: an inline single-line editor, horizontally scrolled to
/// keep the cursor visible when focused.
pub(super) fn form_title_line(form: &IssueForm, t: &Theme, value_width: usize) -> Line<'static> {
    let focused = form.field_idx == 0;
    let style = if focused {
        Style::default().bg(t.selected_bg)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(
        format!(" {:<ISSUE_FORM_LABEL_WIDTH$}", ISSUE_FORM_FIELDS[0]),
        style.fg(t.accent),
    )];
    if focused {
        let skip = input_scroll_skip(form.title.cursor, value_width);
        let visible: String = form
            .title
            .buffer
            .chars()
            .skip(skip)
            .take(value_width)
            .collect();
        spans.extend(
            cursor_spans(&visible, form.title.cursor - skip)
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(t.selected_bg))),
        );
    } else {
        spans.push(Span::raw(form.title.buffer.clone()));
    }
    Line::from(spans)
}

/// The description row: a label line, then a fixed-height, word-wrapped
/// inline block that scrolls to keep the cursor's visual row visible when
/// focused (mirrors the inline comment editor, minus its own border).
pub(super) fn form_description_lines(
    form: &IssueForm,
    t: &Theme,
    value_width: usize,
) -> Vec<Line<'static>> {
    let focused = form.field_idx == 1;
    let label_style = if focused {
        Style::default().bg(t.selected_bg)
    } else {
        Style::default()
    };
    let mut lines = vec![Line::from(Span::styled(
        format!(" {:<ISSUE_FORM_LABEL_WIDTH$}", ISSUE_FORM_FIELDS[1]),
        label_style.fg(t.accent),
    ))];

    let rows = wrap_lines(&form.body.lines, value_width);
    let (cur_row, cur_col) = cursor_row(
        &rows,
        form.body.line,
        form.body.lines[form.body.line].cursor,
    );
    let top = if focused {
        cur_row.saturating_sub(ISSUE_FORM_DESC_HEIGHT.saturating_sub(1))
    } else {
        0
    };
    for i in 0..ISSUE_FORM_DESC_HEIGHT {
        let row_idx = top + i;
        let text = rows
            .get(row_idx)
            .map(|row| {
                form.body.lines[row.line]
                    .buffer
                    .chars()
                    .skip(row.start)
                    .take(row.end - row.start)
                    .collect::<String>()
            })
            .unwrap_or_default();
        let mut spans = vec![Span::raw("   ")];
        if focused && row_idx == cur_row {
            spans.extend(cursor_spans(&text, cur_col));
        } else {
            spans.push(Span::raw(text));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// The centered `[ Create ]  [ Cancel ]` button row, with the focused
/// button drawn in reversed video.
pub(super) fn issue_form_button_line(form: &IssueForm, t: &Theme, width: usize) -> Line<'static> {
    const CREATE: &str = "[ Create ]";
    const CANCEL: &str = "[ Cancel ]";
    let gap = "  ";
    let total = CREATE.len() + gap.len() + CANCEL.len();
    let pad = " ".repeat(width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    Line::from(vec![
        Span::raw(pad),
        Span::styled(
            CREATE,
            button_style(form.field_idx == ISSUE_FORM_CREATE_ROW),
        ),
        Span::raw(gap),
        Span::styled(
            CANCEL,
            button_style(form.field_idx == ISSUE_FORM_CANCEL_ROW),
        ),
    ])
}

/// Option popup for a form field: single-select (with the "—" clear row)
/// or multi-select (Space toggles, checkbox markers).
/// The inline comment section at the bottom of the detail pane
/// (`Mode::CommentEditor`): a multi-line editor with a `[ Save ]  [ Cancel ]`
/// button row, the focused element highlighted. Width matches
/// `comment_pane_width` so the renderer and the key handler's up/down
/// visual-row navigation agree on wrap geometry.
pub(super) fn draw_comment_section(f: &mut Frame, app: &App, t: &Theme, area: Rect) {
    let width = layout::detail_inner_width(f.area().width) as usize;
    // One row reserved for the button line at the bottom of the block.
    let inner_height = area.height.saturating_sub(2) as usize;
    let text_height = inner_height.saturating_sub(1);

    let body = &app.editor.body;
    let rows = wrap_lines(&body.lines, width);
    let (cur_row, cur_col) = cursor_row(&rows, body.line, body.lines[body.line].cursor);
    let top = cur_row.saturating_sub(text_height.saturating_sub(1));
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(top)
        .take(text_height)
        .map(|(i, row)| {
            let text: String = body.lines[row.line]
                .buffer
                .chars()
                .skip(row.start)
                .take(row.end - row.start)
                .collect();
            if i == cur_row && app.editor.focus == CommentFocus::Editor {
                Line::from(cursor_spans(&text, cur_col))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    lines.push(comment_button_line(app, t, width));

    let action = match app.editor.target {
        crate::tui::app::EditorTarget::NewComment => "add comment",
        crate::tui::app::EditorTarget::EditComment { .. } => "edit comment",
        crate::tui::app::EditorTarget::EditBody => "edit description",
    };
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.accent))
            .title(format!(" {action} (Ctrl+S save · Esc cancel) ")),
    );
    f.render_widget(para, area);
}

/// The centered `[ Save ]  [ Cancel ]` button row, with the focused button
/// drawn in reversed video.
pub(super) fn comment_button_line(app: &App, t: &Theme, width: usize) -> Line<'static> {
    const SAVE: &str = "[ Save ]";
    const CANCEL: &str = "[ Cancel ]";
    let gap = "  ";
    let total = SAVE.len() + gap.len() + CANCEL.len();
    let pad = " ".repeat(width.saturating_sub(total) / 2);

    let button_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    Line::from(vec![
        Span::raw(pad),
        Span::styled(SAVE, button_style(app.editor.focus == CommentFocus::Save)),
        Span::raw(gap),
        Span::styled(
            CANCEL,
            button_style(app.editor.focus == CommentFocus::Cancel),
        ),
    ])
}
