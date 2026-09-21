//! Fence and code-span rules, shared by the markdown renderer and the PR-link
//! scanner (#157).
//!
//! Both need to know where code is: the renderer to draw it, the scanner to
//! *skip* it, since a literal example or a hex colour inside code is not a
//! reference. They used to carry separate implementations, and the two
//! disagreed — the renderer closed a four-backtick fence at the first
//! three-backtick line and closed an inline span at the next single backtick,
//! while the scanner never honoured a `\` escape. Each was right about what the
//! other got wrong, so neither could be adopted wholesale.
//!
//! This module shares the **rules**, not a return type. The renderer wants
//! line-oriented fences and char-indexed spans to style; the scanner wants byte
//! ranges to mask. Each keeps its own shape and asks these questions.
//!
//! No dependency on `tui` or `provider`: `provider` never imports `tui`, and
//! this module is what keeps both of them from having to.
//!
//! Rules both sides already agreed on, and which are *not* CommonMark-correct,
//! are deliberately left alone: 4+ spaces of indentation should mean an
//! indented code block rather than a fence, a closing fence may not carry an
//! info string, and a backtick fence's info string may not contain a backtick.
//! Nobody reported them, and fixing them would change behaviour both sides
//! agree on today.

use std::ops::Range;

/// A line that opens a fence: its character (`` ` `` or `~`) and the length of
/// its run, at least 3. Leading whitespace is tolerated, as both sides always did.
pub(crate) fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    ['`', '~'].into_iter().find_map(|c| {
        let n = trimmed.chars().take_while(|&x| x == c).count();
        (n >= 3).then_some((c, n))
    })
}

/// Whether `line` closes a fence opened with `fence_char` and a run of
/// `open_len`: the same character, and a run **at least as long as the opening
/// one**. That last clause is the renderer's missing half — without it a
/// three-backtick line closes a four-backtick fence, which is the standard way
/// to show a fence inside a fence.
pub(crate) fn fence_closes(line: &str, fence_char: char, open_len: usize) -> bool {
    fence_open(line).is_some_and(|(c, n)| c == fence_char && n >= open_len)
}

/// What a backtick at some position turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Backticks {
    /// An inline code span. `content` is the text between the delimiters with
    /// one leading and one trailing space stripped, and `end` is the index just
    /// past the closing run.
    Span { content: Range<usize>, end: usize },
    /// Not a span: literal backticks up to `end`, which the caller should emit
    /// and skip. An unmatched run is skipped **whole** — retrying its second
    /// backtick as a run of one could wrongly pair it with a later single.
    Literal { end: usize },
}

/// Classify the backtick at `chars[at]`.
///
/// - **Escaped**: a backtick preceded by an odd number of backslashes is one
///   literal character. This is the scanner's missing half — it never checked
///   for `\`, so `` \`#123\` `` masked a real reference.
/// - **Otherwise** the whole run opens a span, closed by the next run of
///   **exactly the same length**. That is the renderer's missing half — it
///   closed at the next single backtick, so ``` ``a ` b`` ``` broke.
/// - **Space stripping**: when the content starts and ends with a space and is
///   not all spaces, one of each is dropped, so `` `` `code` `` `` reads as
///   `` `code` `` rather than showing padding.
///
/// Only meaningful when `chars[at]` is a backtick; anything else is a
/// zero-width literal.
pub(crate) fn backticks(chars: &[char], at: usize) -> Backticks {
    if chars.get(at) != Some(&'`') {
        return Backticks::Literal { end: at };
    }
    if is_escaped(chars, at) {
        return Backticks::Literal { end: at + 1 };
    }
    let open = run_len(chars, at);
    let content_start = at + open;
    let mut j = content_start;
    while j < chars.len() {
        if chars[j] != '`' {
            j += 1;
            continue;
        }
        let run = run_len(chars, j);
        if run == open {
            return Backticks::Span {
                content: strip_padding(chars, content_start..j),
                end: j + run,
            };
        }
        j += run;
    }
    Backticks::Literal { end: content_start }
}

fn run_len(chars: &[char], from: usize) -> usize {
    chars[from..].iter().take_while(|&&c| c == '`').count()
}

/// Preceded by an odd number of backslashes.
fn is_escaped(chars: &[char], at: usize) -> bool {
    chars[..at].iter().rev().take_while(|&&c| c == '\\').count() % 2 == 1
}

fn strip_padding(chars: &[char], r: Range<usize>) -> Range<usize> {
    let inner = &chars[r.clone()];
    let padded = inner.len() >= 2
        && inner.first() == Some(&' ')
        && inner.last() == Some(&' ')
        && inner.iter().any(|&c| c != ' ');
    if padded { r.start + 1..r.end - 1 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn span(s: &str, at: usize) -> Option<(String, usize)> {
        let c = chars(s);
        match backticks(&c, at) {
            Backticks::Span { content, end } => Some((c[content].iter().collect(), end)),
            Backticks::Literal { .. } => None,
        }
    }

    // ---- fences ----

    #[test]
    fn a_fence_needs_three_and_records_its_character_and_run() {
        assert_eq!(fence_open("```rust"), Some(('`', 3)));
        assert_eq!(fence_open("~~~~"), Some(('~', 4)));
        assert_eq!(
            fence_open("    ```"),
            Some(('`', 3)),
            "leading whitespace tolerated"
        );
        assert_eq!(fence_open("``"), None);
        assert_eq!(fence_open("plain"), None);
    }

    #[test]
    fn a_fence_closes_only_on_the_same_character_and_a_run_at_least_as_long() {
        assert!(fence_closes("```", '`', 3));
        assert!(fence_closes("`````", '`', 3), "a longer run still closes");
        assert!(!fence_closes("```", '`', 4), "a shorter run does not");
        assert!(
            !fence_closes("~~~", '`', 3),
            "tilde does not close a backtick fence"
        );
        assert!(!fence_closes("not a fence", '`', 3));
    }

    // ---- spans ----

    #[test]
    fn a_span_closes_on_a_run_of_equal_length() {
        assert_eq!(span("`a`", 0), Some(("a".into(), 3)));
        assert_eq!(span("``a ` b``", 0), Some(("a ` b".into(), 9)));
        assert_eq!(span("x `a` y", 2), Some(("a".into(), 5)));
    }

    #[test]
    fn a_run_of_a_different_length_does_not_close_it() {
        // ``a` — the closing run is one backtick, the opener is two.
        assert_eq!(span("``a`", 0), None);
        // `a`` — the opener is one, the closing run is two.
        assert_eq!(span("`a``", 0), None);
    }

    #[test]
    fn an_unmatched_run_is_literal_and_skipped_whole() {
        let c = chars("``x");
        assert_eq!(backticks(&c, 0), Backticks::Literal { end: 2 });
        // Skipping only one backtick would let the second retry as a run of
        // one and pair with the later single backtick.
        let c = chars("``a`");
        assert_eq!(backticks(&c, 0), Backticks::Literal { end: 2 });
    }

    #[test]
    fn an_escaped_backtick_is_one_literal_character() {
        let c = chars("\\`x`");
        assert_eq!(backticks(&c, 1), Backticks::Literal { end: 2 });
    }

    #[test]
    fn an_even_number_of_backslashes_does_not_escape() {
        // `\\` is an escaped backslash, so the backtick that follows is real.
        assert_eq!(span("\\\\`a`", 2), Some(("a".into(), 5)));
        // Three backslashes: the last escapes the backtick.
        let c = chars("\\\\\\`a`");
        assert_eq!(backticks(&c, 3), Backticks::Literal { end: 4 });
    }

    #[test]
    fn one_padding_space_is_stripped_from_each_side() {
        assert_eq!(span("`` `code` ``", 0), Some(("`code`".into(), 12)));
        // Only one each: the rest is content.
        assert_eq!(span("`  a  `", 0), Some((" a ".into(), 7)));
    }

    #[test]
    fn padding_is_kept_when_only_one_side_has_it_or_all_is_space() {
        assert_eq!(span("` a`", 0), Some((" a".into(), 4)));
        assert_eq!(span("`a `", 0), Some(("a ".into(), 4)));
        assert_eq!(
            span("`   `", 0),
            Some(("   ".into(), 5)),
            "all spaces is content"
        );
    }

    #[test]
    fn a_non_backtick_position_is_a_zero_width_literal() {
        let c = chars("abc");
        assert_eq!(backticks(&c, 1), Backticks::Literal { end: 1 });
        assert_eq!(backticks(&c, 9), Backticks::Literal { end: 9 });
    }
}
