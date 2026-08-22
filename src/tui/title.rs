//! The outer terminal's window title (#132).
//!
//! A harness session is drawn full-frame, so when the TUI is not the focused
//! pane there is nothing at all saying which issue an agent is working. The
//! window/tab title carries that where the identity row cannot reach.
//!
//! Two things this deliberately does not attempt:
//!
//! - **Reading the previous title back.** There is no portable escape for it
//!   (the `21t` query is widely disabled as a security measure), so restore
//!   writes a fixed name rather than whatever was there before.
//! - **Defending the title against the agent.** A child may emit its own
//!   `OSC 2` mid-session and win. The title is a secondary signal; the
//!   identity row above the pane is the one that cannot be overwritten.

use std::io::Write;

/// What the title says with no session attached.
const IDLE: &str = "gh-issues-tui";

/// Sets the terminal title, and puts it back on the way out.
///
/// `Drop` covers the normal exit and unwinding panics; the panic hook in
/// `main` covers the rest, since a hook runs before unwinding reaches here.
pub struct TerminalTitle {
    /// What was last written, so a redraw-per-keystroke does not re-emit the
    /// same escape sequence sixty times a second.
    current: Option<String>,
}

impl TerminalTitle {
    pub fn new() -> Self {
        let mut t = Self { current: None };
        t.set(IDLE);
        t
    }

    /// Point the title at `issue_ref`, or back at the app name when `None`.
    pub fn sync(&mut self, issue_ref: Option<&str>) {
        self.set(&desired(issue_ref));
    }

    fn set(&mut self, title: &str) {
        if let Some(clean) = changed(self.current.as_deref(), title) {
            emit(&clean);
            self.current = Some(clean);
        }
    }
}

/// What the title should read for this session state.
fn desired(issue_ref: Option<&str>) -> String {
    match issue_ref {
        Some(r) => format!("{IDLE} \u{00b7} {r}"),
        None => IDLE.to_string(),
    }
}

/// The sanitized title to write, or `None` if it is already on screen.
///
/// Split out from [`TerminalTitle::set`] so the dedup that keeps a
/// redraw-per-keystroke loop from re-emitting the same escape sequence can be
/// tested without writing escape sequences into the test harness's output.
fn changed(current: Option<&str>, title: &str) -> Option<String> {
    let clean = sanitize(title);
    (current != Some(clean.as_str())).then_some(clean)
}

impl Drop for TerminalTitle {
    fn drop(&mut self) {
        emit(IDLE);
    }
}

impl Default for TerminalTitle {
    fn default() -> Self {
        Self::new()
    }
}

/// Best-effort: a terminal that ignores `OSC 2`, or a closed stdout, is not
/// worth failing a redraw over.
fn emit(title: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\u{1b}]2;{title}\u{7}");
    let _ = out.flush();
}

/// Restore the title from anywhere, including a panic hook that cannot reach
/// the guard.
pub fn restore() {
    emit(IDLE);
}

/// Strip anything that could end the escape sequence early or start another.
///
/// Repo names and issue titles come from the API, so this string is not ours.
/// A `BEL` would terminate the `OSC` early and leave the rest to be
/// interpreted as commands; an `ESC` could open a fresh sequence. Dropping
/// every control character closes both, and the length cap keeps a pathological
/// title from filling a tab bar.
fn sanitize(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_the_sequence_terminator() {
        // The attack this pins: a repo or issue title carrying a BEL ends the
        // OSC, and everything after it reaches the terminal as its own input.
        let got = sanitize("repo\u{7}evil");
        assert_eq!(got, "repoevil");
        assert!(!got.contains('\u{7}'));
    }

    #[test]
    fn sanitize_drops_escape_so_a_new_sequence_cannot_be_opened() {
        assert_eq!(sanitize("a\u{1b}]0;b\u{7}c"), "a]0;bc");
    }

    #[test]
    fn sanitize_drops_newlines_and_carriage_returns() {
        assert_eq!(sanitize("one\ntwo\rthree"), "onetwothree");
    }

    #[test]
    fn sanitize_caps_a_pathological_title() {
        let long = "x".repeat(500);
        assert_eq!(sanitize(&long).chars().count(), 128);
    }

    #[test]
    fn sanitize_keeps_ordinary_issue_refs_intact() {
        assert_eq!(
            sanitize("gh-issues-tui \u{00b7} pgmac-net/gh-issues-tui#132"),
            "gh-issues-tui \u{00b7} pgmac-net/gh-issues-tui#132"
        );
    }

    #[test]
    fn an_attached_session_puts_its_issue_in_the_title() {
        assert_eq!(
            desired(Some("pgmac-net/gh-issues-tui#132")),
            "gh-issues-tui \u{00b7} pgmac-net/gh-issues-tui#132"
        );
    }

    #[test]
    fn detaching_returns_the_title_to_the_app_name() {
        assert_eq!(desired(None), "gh-issues-tui");
    }

    #[test]
    fn an_unchanged_title_is_not_rewritten() {
        // The loop calls `sync` before every frame; without this it would
        // emit an escape sequence per keystroke.
        assert_eq!(changed(Some("gh-issues-tui"), "gh-issues-tui"), None);
    }

    #[test]
    fn a_changed_title_comes_back_sanitized() {
        assert_eq!(
            changed(Some("gh-issues-tui"), "gh-issues-tui \u{00b7} o/r#1\u{7}"),
            Some("gh-issues-tui \u{00b7} o/r#1".to_string())
        );
    }

    #[test]
    fn dedup_compares_the_sanitized_form_not_the_raw_one() {
        // Two titles differing only in stripped control characters render
        // identically, so the second must not be rewritten.
        assert_eq!(changed(Some("abc"), "a\u{7}bc"), None);
    }
}
