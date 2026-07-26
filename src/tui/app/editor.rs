use super::mode::{CommentFocus, EditorTarget};

#[derive(Debug, Default)]
pub struct InputState {
    pub buffer: String,
    pub cursor: usize,
}

impl InputState {
    pub fn start(&mut self, initial: &str) {
        self.buffer = initial.to_string();
        self.cursor = self.buffer.chars().count();
    }

    pub fn insert(&mut self, c: char) {
        let byte = self.byte_at(self.cursor);
        self.buffer.insert(byte, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte = self.byte_at(self.cursor);
            self.buffer.remove(byte);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.buffer.chars().count());
    }

    /// Delete/Ctrl+D: remove the character under the cursor.
    pub fn delete_char(&mut self) {
        if self.cursor < self.buffer.chars().count() {
            let byte = self.byte_at(self.cursor);
            self.buffer.remove(byte);
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    /// Ctrl+←: to the start of the current or previous word
    /// (whitespace-delimited).
    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Ctrl+→: to the end of the current or next word.
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Ctrl+W: delete the word before the cursor (and the whitespace
    /// between it and the cursor).
    pub fn delete_word_back(&mut self) {
        let end = self.byte_at(self.cursor);
        self.word_left();
        let start = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
    }

    /// Ctrl+U: delete from the cursor back to the start of the line.
    pub fn kill_to_start(&mut self) {
        let byte = self.byte_at(self.cursor);
        self.buffer.replace_range(..byte, "");
        self.cursor = 0;
    }

    /// Ctrl+K: delete from the cursor to the end of the line.
    pub fn kill_to_end(&mut self) {
        let byte = self.byte_at(self.cursor);
        self.buffer.truncate(byte);
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }

    /// Split at the cursor: everything before stays, the tail is returned.
    fn split_at_cursor(&mut self) -> String {
        let byte = self.byte_at(self.cursor);
        self.buffer.split_off(byte)
    }
}

/// Minimal multi-line editor for the issue body: one UTF-8-safe
/// `InputState` per line. Always holds at least one line.
#[derive(Debug)]
pub struct BodyEditor {
    pub lines: Vec<InputState>,
    /// Index of the line the cursor is on.
    pub line: usize,
}

impl Default for BodyEditor {
    fn default() -> Self {
        Self {
            lines: vec![InputState::default()],
            line: 0,
        }
    }
}

impl BodyEditor {
    /// Prefill an editor with existing text, one `InputState` per line, cursor
    /// at the end of the last line. An empty string yields the `Default`
    /// single blank line.
    pub fn from_text(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        let lines: Vec<InputState> = text
            .split('\n')
            .map(|l| InputState {
                cursor: l.chars().count(),
                buffer: l.to_string(),
            })
            .collect();
        let line = lines.len() - 1;
        Self { lines, line }
    }

    pub fn insert(&mut self, c: char) {
        self.lines[self.line].insert(c);
    }

    /// Enter: split the current line at the cursor.
    pub fn newline(&mut self) {
        let tail = self.lines[self.line].split_at_cursor();
        self.line += 1;
        self.lines.insert(
            self.line,
            InputState {
                buffer: tail,
                cursor: 0,
            },
        );
    }

    /// Backspace: within a line deletes a char; at column 0 merges the
    /// line into the previous one.
    pub fn backspace(&mut self) {
        if self.lines[self.line].cursor > 0 {
            self.lines[self.line].backspace();
        } else if self.line > 0 {
            let removed = self.lines.remove(self.line);
            self.line -= 1;
            let prev = &mut self.lines[self.line];
            prev.cursor = prev.buffer.chars().count();
            prev.buffer.push_str(&removed.buffer);
        }
    }

    /// Delete/Ctrl+D: within a line deletes the char under the cursor; at
    /// the end of a line merges the next line up (mirror of backspace).
    pub fn delete_char(&mut self) {
        let cur = &self.lines[self.line];
        if cur.cursor < cur.buffer.chars().count() {
            self.lines[self.line].delete_char();
        } else if self.line + 1 < self.lines.len() {
            let removed = self.lines.remove(self.line + 1);
            self.lines[self.line].buffer.push_str(&removed.buffer);
        }
    }

    pub fn left(&mut self) {
        self.lines[self.line].left();
    }

    pub fn right(&mut self) {
        self.lines[self.line].right();
    }

    pub fn word_left(&mut self) {
        self.lines[self.line].word_left();
    }

    pub fn word_right(&mut self) {
        self.lines[self.line].word_right();
    }

    pub fn delete_word_back(&mut self) {
        self.lines[self.line].delete_word_back();
    }

    pub fn home(&mut self) {
        self.lines[self.line].home();
    }

    pub fn end(&mut self) {
        self.lines[self.line].end();
    }

    pub fn kill_to_start(&mut self) {
        self.lines[self.line].kill_to_start();
    }

    pub fn kill_to_end(&mut self) {
        self.lines[self.line].kill_to_end();
    }

    /// Up one *visual* row of the `width`-wrapped layout, clamping the
    /// column; a no-op on the first row.
    pub fn up_visual(&mut self, width: usize) {
        self.move_visual(width, -1);
    }

    /// Down one visual row; a no-op on the last.
    pub fn down_visual(&mut self, width: usize) {
        self.move_visual(width, 1);
    }

    fn move_visual(&mut self, width: usize, delta: isize) {
        let rows = wrap_lines(&self.lines, width);
        let (row_idx, col) = cursor_row(&rows, self.line, self.lines[self.line].cursor);
        let Some(target) = row_idx
            .checked_add_signed(delta)
            .filter(|t| *t < rows.len())
        else {
            return;
        };
        let row = rows[target];
        // On a non-final row the position `end` already belongs to the next
        // visual row, so the rightmost landing spot is one before it.
        let line_final = rows.get(target + 1).is_none_or(|n| n.line != row.line);
        let max = if line_final {
            row.end
        } else {
            row.end.saturating_sub(1)
        };
        self.line = row.line;
        self.lines[self.line].cursor = (row.start + col).min(max);
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.buffer.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One-line summary for the form row.
    pub fn summary(&self) -> String {
        let text = self.text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let first = trimmed.lines().next().unwrap_or_default();
        let extra = trimmed.lines().count().saturating_sub(1);
        if extra > 0 {
            format!("{first} (+{extra} more lines)")
        } else {
            first.to_string()
        }
    }
}

/// The single-line input popup's outer width; inner width mirrors
/// `issue_form_width`'s clamp-minus-borders pattern.
pub const INPUT_POPUP_WIDTH: u16 = 60;

pub fn input_popup_width(frame_width: u16) -> u16 {
    INPUT_POPUP_WIDTH.min(frame_width).saturating_sub(2)
}

/// The char index to start displaying from so a single-line input's cursor
/// always stays within a `width`-wide window. Stateless: recomputed from
/// `cursor` and `width` each frame, so the window only moves when the
/// cursor's position relative to the current window requires it.
pub fn input_scroll_skip(cursor: usize, width: usize) -> usize {
    let width = width.max(1);
    cursor.saturating_sub(width.saturating_sub(1))
}

/// One visual row of the word-wrapped body: a char range of a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    /// Index into `BodyEditor::lines`.
    pub line: usize,
    /// Char range within that line (`start..end`).
    pub start: usize,
    pub end: usize,
}

/// Word-wrap every logical line at `width` chars: break after the last
/// whitespace fitting in the window, hard-break words longer than `width`.
/// An empty line yields one empty row; `width` of 0 is treated as 1.
pub fn wrap_lines(lines: &[InputState], width: usize) -> Vec<VisualRow> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for (line_idx, l) in lines.iter().enumerate() {
        let chars: Vec<char> = l.buffer.chars().collect();
        let mut start = 0;
        loop {
            if chars.len() - start <= width {
                rows.push(VisualRow {
                    line: line_idx,
                    start,
                    end: chars.len(),
                });
                break;
            }
            let window_end = start + width;
            let brk = (start..window_end)
                .rev()
                .find(|&i| chars[i].is_whitespace())
                .map(|i| i + 1) // the space stays on this row
                .unwrap_or(window_end); // no space: hard break
            rows.push(VisualRow {
                line: line_idx,
                start,
                end: brk,
            });
            start = brk;
        }
    }
    rows
}

/// The visual position of a cursor: `(row index, column within the row)`.
/// A cursor sitting exactly on a wrap boundary belongs to the start of the
/// following row, except at the very end of a logical line.
pub fn cursor_row(rows: &[VisualRow], line: usize, cursor: usize) -> (usize, usize) {
    for (idx, row) in rows.iter().enumerate() {
        if row.line != line {
            continue;
        }
        let line_final = rows.get(idx + 1).is_none_or(|next| next.line != line);
        if cursor < row.end || (cursor == row.end && line_final) {
            return (idx, cursor - row.start);
        }
    }
    (0, 0) // unreachable with a clamped cursor; safe fallback
}

/// The inline comment/description editor's session state: what is being
/// typed, which element of the widget has keys, and what a save writes.
///
/// All three are set together each time the editor opens and cleared
/// together when it closes, so `Default` is "editor closed, nothing pending".
#[derive(Debug, Default)]
pub struct EditorState {
    /// The multi-line editor backing `Mode::CommentEditor`.
    pub body: BodyEditor,
    /// Which element of the comment section has keys.
    pub focus: CommentFocus,
    /// What a save writes: a new comment, an edit to one, or the issue body.
    pub target: EditorTarget,
}

impl EditorState {
    /// Open the editor on `body`, writing to `target` on save.
    pub fn start(&mut self, body: BodyEditor, target: EditorTarget) {
        self.body = body;
        self.focus = CommentFocus::Editor;
        self.target = target;
    }

    /// Take the composed text and reset to closed, in one step — the caller
    /// then decides what to do with it. Resetting here rather than at each
    /// call site is what stops a half-cleared editor leaking into the next
    /// one.
    pub fn take(&mut self) -> (String, EditorTarget) {
        let text = self.body.text();
        let target = std::mem::take(self);
        (text, target.target)
    }
}
