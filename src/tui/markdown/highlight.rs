//! In-house per-language token highlighting for fenced code (#122).
//!
//! One generic forward scanner ([`tokenize`]) driven by a per-language
//! [`LangSpec`] table. Deliberately *not* a grammar engine: `syntect` was
//! rejected on #120 because it drags in a C or backtracking-regex dependency
//! plus ~2MB of syntax dumps across four release platforms.
//!
//! The scanner emits contiguous, non-empty byte ranges covering the whole
//! line, so [`super::fence`] can turn them straight into styled spans and then
//! hard-break *those* — tokenising the full source line before the pane-width
//! split is what keeps colours identical at every pane width.
//!
//! ## Deliberately unhandled
//!
//! Heredocs, backslash line continuations, JSX, template-literal
//! interpolation, Rust raw strings (`r#"…"#`), Rust lifetimes vs char literals
//! (`'` is not a Rust string delimiter here, so `'x'` reads as plain text
//! rather than letting `'a` colour the rest of the line), and numeric type
//! suffixes (`1u32` is a number followed by an identifier). Each degrades to
//! plain `code_fg`, never to a colour that runs away down the block.

use std::ops::Range;

/// What a scanned range is, and hence which theme colour it draws in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenKind {
    /// A language keyword, and the structural key of a `json`/`yaml`/`toml`
    /// mapping entry — those share one colour rather than earning a fifth.
    Keyword,
    Str,
    Comment,
    Number,
    /// Everything else: identifiers, punctuation, whitespace.
    Text,
}

/// One string-literal form.
#[derive(Debug, Clone, Copy)]
struct StringRule {
    delim: char,
    /// `None` for raw forms where a backslash is literal (shell/TOML `'…'`,
    /// Go backticks).
    escape: Option<char>,
}

const fn esc(delim: char) -> StringRule {
    StringRule {
        delim,
        escape: Some('\\'),
    }
}

const fn raw(delim: char) -> StringRule {
    StringRule {
        delim,
        escape: None,
    }
}

/// The per-language rule table the generic scanner runs on.
pub(super) struct LangSpec {
    keywords: &'static [&'static str],
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Checked before [`Self::strings`], so `"""` wins over `"`.
    triple: &'static [&'static str],
    strings: &'static [StringRule],
    /// Non-empty for the structural languages: the char that marks the end of
    /// a mapping key at the head of a line (`:` for yaml/json, `=` for toml).
    key_terminators: &'static [char],
}

/// A construct left open by the previous line of the *same* fence. Reset to
/// [`State::Normal`] for every new fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Normal,
    BlockComment,
    /// Inside a triple-quoted string opened with this exact delimiter.
    Triple(&'static str),
}

/// Resolve a fence's info string to a spec: first word, lowercased, through
/// the alias table. `None` — including for `console`/`shell-session`, which
/// are prompt-and-output transcripts rather than source — leaves the block
/// with its pre-#122 flat styling.
pub(super) fn spec_for(info: &str) -> Option<&'static LangSpec> {
    let word = info
        .split([',', ' ', '\t', '{'])
        .next()?
        .trim()
        .to_ascii_lowercase();
    match word.as_str() {
        "sh" | "bash" | "zsh" | "ksh" | "shell" => Some(&SH),
        "yaml" | "yml" => Some(&YAML),
        "json" => Some(&JSON),
        "toml" => Some(&TOML),
        "rust" | "rs" => Some(&RUST),
        "python" | "py" => Some(&PYTHON),
        "go" | "golang" => Some(&GO),
        "js" | "jsx" | "mjs" | "cjs" | "javascript" | "ts" | "tsx" | "typescript" => Some(&JSTS),
        _ => None,
    }
}

/// Scan one source line, advancing `st` across any construct it leaves open.
///
/// The returned ranges are non-empty, ascending, adjacent, and together cover
/// `0..line.len()`.
pub(super) fn tokenize(
    line: &str,
    spec: &LangSpec,
    st: &mut State,
) -> Vec<(Range<usize>, TokenKind)> {
    let started_normal = *st == State::Normal;
    let mut out: Vec<(Range<usize>, TokenKind)> = Vec::new();
    let mut i = 0usize;

    // Resume whatever the previous line left open before scanning normally.
    match *st {
        State::BlockComment => {
            let close = spec.block_comment.map(|(_, c)| c).unwrap_or("*/");
            match line.find(close) {
                Some(p) => {
                    let end = p + close.len();
                    push(&mut out, 0..end, TokenKind::Comment);
                    i = end;
                    *st = State::Normal;
                }
                None => {
                    push(&mut out, 0..line.len(), TokenKind::Comment);
                    return out;
                }
            }
        }
        State::Triple(q) => match line.find(q) {
            Some(p) => {
                let end = p + q.len();
                push(&mut out, 0..end, TokenKind::Str);
                i = end;
                *st = State::Normal;
            }
            None => {
                push(&mut out, 0..line.len(), TokenKind::Str);
                return out;
            }
        },
        State::Normal => {}
    }

    // Start of the run of plain text currently being accumulated, if any.
    let mut text: Option<usize> = None;

    while i < line.len() {
        let rest = &line[i..];

        if spec.line_comment.iter().any(|p| rest.starts_with(p)) {
            flush(&mut out, &mut text, i);
            push(&mut out, i..line.len(), TokenKind::Comment);
            break;
        }

        if let Some((open, close)) = spec.block_comment
            && rest.starts_with(open)
        {
            flush(&mut out, &mut text, i);
            match line[i + open.len()..].find(close) {
                Some(p) => {
                    let end = i + open.len() + p + close.len();
                    push(&mut out, i..end, TokenKind::Comment);
                    i = end;
                }
                None => {
                    push(&mut out, i..line.len(), TokenKind::Comment);
                    *st = State::BlockComment;
                    i = line.len();
                }
            }
            continue;
        }

        if let Some(q) = first_prefix(spec.triple, rest) {
            flush(&mut out, &mut text, i);
            match line[i + q.len()..].find(q) {
                Some(p) => {
                    let end = i + q.len() + p + q.len();
                    push(&mut out, i..end, TokenKind::Str);
                    i = end;
                }
                None => {
                    push(&mut out, i..line.len(), TokenKind::Str);
                    *st = State::Triple(q);
                    i = line.len();
                }
            }
            continue;
        }

        let c = rest.chars().next().expect("i < line.len()");

        if let Some(rule) = spec.strings.iter().find(|r| r.delim == c).copied() {
            flush(&mut out, &mut text, i);
            let end = scan_string(line, i, rule);
            push(&mut out, i..end, TokenKind::Str);
            i = end;
            continue;
        }

        if c.is_ascii_digit() {
            flush(&mut out, &mut text, i);
            let end = scan_number(line, i);
            push(&mut out, i..end, TokenKind::Number);
            i = end;
            continue;
        }

        if is_ident_start(c) {
            let end = scan_ident(line, i);
            if spec.keywords.contains(&&line[i..end]) {
                flush(&mut out, &mut text, i);
                push(&mut out, i..end, TokenKind::Keyword);
            } else if text.is_none() {
                // A plain identifier is indistinguishable from surrounding
                // punctuation, so it folds into one Text run.
                text = Some(i);
            }
            i = end;
            continue;
        }

        if text.is_none() {
            text = Some(i);
        }
        i += c.len_utf8();
    }
    flush(&mut out, &mut text, line.len());

    if started_normal {
        retag_key(line, spec, &mut out);
    }
    out
}

/// Retag a leading `key:` / `key =` as [`TokenKind::Keyword`] for the
/// structural languages, so a JSON/YAML/TOML block isn't one flat colour.
///
/// Works on the byte range rather than on whole tokens so that hyphenated
/// (`key-name:`), dotted (`a.b.c =`) and quoted (`"key":`) keys are all one
/// rule. String tokens are stepped over, so a `:` *inside* a quoted key
/// doesn't terminate it; a comment token ends the search.
fn retag_key(line: &str, spec: &LangSpec, tokens: &mut Vec<(Range<usize>, TokenKind)>) {
    if spec.key_terminators.is_empty() {
        return;
    }

    let mut start = line.len() - line.trim_start().len();
    // A YAML list item's `- ` is punctuation, not part of the key.
    if line[start..].starts_with("- ") {
        start += 2;
        start += line[start..].len() - line[start..].trim_start().len();
    }

    let mut p = start;
    let mut found = None;
    while p < line.len() {
        match kind_at(tokens, p) {
            Some((r, TokenKind::Str)) => {
                p = r.end;
                continue;
            }
            Some((_, TokenKind::Comment)) => break,
            _ => {}
        }
        let c = line[p..].chars().next().expect("p < line.len()");
        if spec.key_terminators.contains(&c) {
            found = Some(p);
            break;
        }
        p += c.len_utf8();
    }

    // Whitespace before the terminator (`a.b.c = 1`) is not part of the key.
    let Some(end) = found else { return };
    let end = start + line[start..end].trim_end().len();
    if end <= start {
        return;
    }

    // Clip every token back out of `start..end`, then drop the key in.
    let mut rebuilt = Vec::with_capacity(tokens.len() + 2);
    for (r, k) in tokens.drain(..) {
        if r.end <= start || r.start >= end {
            rebuilt.push((r, k));
            continue;
        }
        if r.start < start {
            rebuilt.push((r.start..start, k));
        }
        if r.end > end {
            rebuilt.push((end..r.end, k));
        }
    }
    rebuilt.push((start..end, TokenKind::Keyword));
    rebuilt.sort_by_key(|(r, _)| r.start);
    *tokens = rebuilt;
}

/// The token covering byte offset `p`, if any.
fn kind_at(tokens: &[(Range<usize>, TokenKind)], p: usize) -> Option<(Range<usize>, TokenKind)> {
    tokens
        .iter()
        .find(|(r, _)| r.contains(&p))
        .map(|(r, k)| (r.clone(), *k))
}

fn first_prefix(prefixes: &'static [&'static str], rest: &str) -> Option<&'static str> {
    prefixes.iter().find(|p| rest.starts_with(**p)).copied()
}

fn push(out: &mut Vec<(Range<usize>, TokenKind)>, r: Range<usize>, k: TokenKind) {
    if !r.is_empty() {
        out.push((r, k));
    }
}

fn flush(out: &mut Vec<(Range<usize>, TokenKind)>, text: &mut Option<usize>, upto: usize) {
    if let Some(start) = text.take() {
        push(out, start..upto, TokenKind::Text);
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn scan_ident(line: &str, start: usize) -> usize {
    let mut i = start;
    while let Some(c) = line[i..].chars().next() {
        if !is_ident_char(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

/// Consume a string literal from its opening delimiter. An *unterminated*
/// literal ends at end-of-line rather than bleeding into the next one — a
/// stray quote in a code sample must not recolour the rest of the block.
fn scan_string(line: &str, start: usize, rule: StringRule) -> usize {
    let mut i = start + rule.delim.len_utf8();
    while let Some(c) = line[i..].chars().next() {
        if Some(c) == rule.escape {
            i += c.len_utf8();
            if let Some(next) = line[i..].chars().next() {
                i += next.len_utf8();
            }
            continue;
        }
        i += c.len_utf8();
        if c == rule.delim {
            return i;
        }
    }
    line.len()
}

/// Consume a numeric literal: `0x`/`0b`/`0o` runs, decimals with `_`
/// separators, a fractional part, and an exponent. Type suffixes are left to
/// the identifier scanner.
fn scan_number(line: &str, start: usize) -> usize {
    let b = line.as_bytes();
    let mut i = start;

    if b[i] == b'0' && i + 1 < b.len() && matches!(b[i + 1] | 32, b'x' | b'b' | b'o') {
        i += 2;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        return i;
    }

    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    if i < b.len() && b[i] | 32 == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

// ---------------------------------------------------------------------------
// Language tables
// ---------------------------------------------------------------------------

static SH: LangSpec = LangSpec {
    keywords: &[
        "if", "then", "elif", "else", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "in", "function", "return", "local", "export", "readonly", "declare", "unset", "shift",
        "break", "continue", "exit", "set", "trap", "eval", "exec", "source", "select", "time",
    ],
    line_comment: &["#"],
    block_comment: None,
    triple: &[],
    strings: &[esc('"'), raw('\'')],
    key_terminators: &[],
};

static YAML: LangSpec = LangSpec {
    keywords: &[
        "true", "false", "null", "yes", "no", "on", "off", "True", "False", "Null", "TRUE",
        "FALSE", "NULL",
    ],
    line_comment: &["#"],
    block_comment: None,
    triple: &[],
    strings: &[esc('"'), raw('\'')],
    key_terminators: &[':'],
};

static JSON: LangSpec = LangSpec {
    keywords: &["true", "false", "null"],
    line_comment: &[],
    block_comment: None,
    triple: &[],
    strings: &[esc('"')],
    key_terminators: &[':'],
};

static TOML: LangSpec = LangSpec {
    keywords: &["true", "false"],
    line_comment: &["#"],
    block_comment: None,
    triple: &["\"\"\"", "'''"],
    strings: &[esc('"'), raw('\'')],
    key_terminators: &['='],
};

static RUST: LangSpec = LangSpec {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    triple: &[],
    // No `'`: a lifetime would otherwise open a string that never closes.
    strings: &[esc('"')],
    key_terminators: &[],
};

static PYTHON: LangSpec = LangSpec {
    keywords: &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "self", "try", "while", "with", "yield",
    ],
    line_comment: &["#"],
    block_comment: None,
    triple: &["\"\"\"", "'''"],
    strings: &[esc('"'), esc('\'')],
    key_terminators: &[],
};

static GO: LangSpec = LangSpec {
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
        "true",
        "false",
        "nil",
        "iota",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    triple: &[],
    strings: &[esc('"'), raw('`'), esc('\'')],
    key_terminators: &[],
};

/// One spec for JavaScript and TypeScript: the keyword list is the union, so a
/// plain `.js` block showing `interface` in keyword colour is the accepted
/// cost of not maintaining two near-identical tables.
static JSTS: LangSpec = LangSpec {
    keywords: &[
        "abstract",
        "any",
        "as",
        "asserts",
        "async",
        "await",
        "bigint",
        "boolean",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "infer",
        "instanceof",
        "interface",
        "is",
        "keyof",
        "let",
        "namespace",
        "never",
        "new",
        "null",
        "number",
        "object",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "set",
        "static",
        "string",
        "super",
        "switch",
        "symbol",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "unique",
        "unknown",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    triple: &[],
    strings: &[esc('"'), esc('\''), esc('`')],
    key_terminators: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokenise a single standalone line, returning `(text, kind)` pairs.
    fn toks(lang: &str, line: &str) -> Vec<(String, TokenKind)> {
        let spec = spec_for(lang).expect("test langs resolve");
        let mut st = State::Normal;
        tokenize(line, spec, &mut st)
            .into_iter()
            .map(|(r, k)| (line[r].to_string(), k))
            .collect()
    }

    /// Every token of `kind`, in order.
    fn of_kind(lang: &str, line: &str, kind: TokenKind) -> Vec<String> {
        toks(lang, line)
            .into_iter()
            .filter(|(_, k)| *k == kind)
            .map(|(s, _)| s)
            .collect()
    }

    #[test]
    fn ranges_are_contiguous_and_cover_every_line() {
        let cases = [
            ("rust", "let x = \"hi\"; // done"),
            ("python", "def f(a=1):  # doc"),
            ("json", "  {\"a\": [1, true, null]}"),
            ("yaml", "- name: value # c"),
            ("toml", "key = 'raw'"),
            ("sh", "if [ -n \"$x\" ]; then echo 1; fi"),
            ("go", "func main() { /* x */ }"),
            ("ts", "const a: number = 0x1f;"),
            ("rust", ""),
            ("rust", "   "),
        ];
        for (lang, line) in cases {
            let spec = spec_for(lang).unwrap();
            let mut st = State::Normal;
            let out = tokenize(line, spec, &mut st);
            let mut at = 0;
            for (r, _) in &out {
                assert_eq!(r.start, at, "gap or overlap in {lang:?} {line:?}");
                assert!(!r.is_empty(), "empty range in {lang:?} {line:?}");
                at = r.end;
            }
            assert_eq!(at, line.len(), "did not cover {lang:?} {line:?}");
        }
    }

    #[test]
    fn rust_keywords_strings_numbers_and_line_comment() {
        let line = "let n = 42; // note";
        assert_eq!(of_kind("rust", line, TokenKind::Keyword), ["let"]);
        assert_eq!(of_kind("rust", line, TokenKind::Number), ["42"]);
        assert_eq!(of_kind("rust", line, TokenKind::Comment), ["// note"]);
        assert_eq!(
            of_kind("rust", "let s = \"hi\";", TokenKind::Str),
            ["\"hi\""]
        );
    }

    #[test]
    fn keyword_is_not_matched_inside_a_longer_identifier() {
        assert!(of_kind("rust", "let iffy = ifs + selfish;", TokenKind::Keyword) == ["let"]);
    }

    #[test]
    fn rust_lifetime_does_not_open_a_string() {
        // `'a` must stay plain text, or it would colour the rest of the line.
        let line = "fn f<'a>(x: &'a str) -> &'a str { x }";
        assert!(of_kind("rust", line, TokenKind::Str).is_empty());
    }

    #[test]
    fn escaped_quote_does_not_close_a_string() {
        assert_eq!(
            of_kind("rust", r#"let s = "a\"b"; let t = 1;"#, TokenKind::Str),
            [r#""a\"b""#]
        );
    }

    #[test]
    fn shell_single_quotes_are_raw() {
        // A backslash inside '…' is literal, so the string still ends at the
        // next quote.
        assert_eq!(of_kind("sh", r"echo 'a\' b", TokenKind::Str), [r"'a\'"]);
    }

    #[test]
    fn unterminated_string_stops_at_end_of_line() {
        let spec = spec_for("rust").unwrap();
        let mut st = State::Normal;
        let out = tokenize("let s = \"oops;", spec, &mut st);
        assert_eq!(out.last().unwrap().1, TokenKind::Str);
        // Crucially, no state is carried: the next line scans normally.
        assert_eq!(st, State::Normal);
    }

    #[test]
    fn block_comment_carries_across_three_lines() {
        let spec = spec_for("go").unwrap();
        let mut st = State::Normal;
        let l1 = tokenize("/* one", spec, &mut st);
        assert_eq!(st, State::BlockComment);
        assert_eq!(l1, vec![(0..6, TokenKind::Comment)]);

        let l2 = tokenize("   two", spec, &mut st);
        assert_eq!(st, State::BlockComment);
        assert_eq!(l2, vec![(0..6, TokenKind::Comment)]);

        let l3 = tokenize("*/ var x = 1", spec, &mut st);
        assert_eq!(st, State::Normal);
        assert_eq!(l3[0], (0..2, TokenKind::Comment));
        assert!(
            l3.iter()
                .any(|(r, k)| *k == TokenKind::Keyword && &"*/ var x = 1"[r.clone()] == "var")
        );
    }

    #[test]
    fn block_comment_opened_and_closed_on_one_line() {
        let line = "var a = 1 /* c */ + 2";
        assert_eq!(of_kind("go", line, TokenKind::Comment), ["/* c */"]);
        assert_eq!(of_kind("go", line, TokenKind::Number), ["1", "2"]);
    }

    #[test]
    fn python_triple_quote_carries_across_lines() {
        let spec = spec_for("python").unwrap();
        let mut st = State::Normal;
        tokenize("    \"\"\"Does a thing.", spec, &mut st);
        assert_eq!(st, State::Triple("\"\"\""));
        let mid = tokenize("    Longer prose.", spec, &mut st);
        assert_eq!(mid, vec![(0..17, TokenKind::Str)]);
        tokenize("    \"\"\"", spec, &mut st);
        assert_eq!(st, State::Normal);
    }

    #[test]
    fn triple_quote_opened_and_closed_on_one_line() {
        assert_eq!(
            of_kind("python", "d = \"\"\"one line\"\"\"", TokenKind::Str),
            ["\"\"\"one line\"\"\""]
        );
    }

    #[test]
    fn json_key_takes_keyword_colour_and_value_string_does_not() {
        let t = toks("json", "  \"name\": \"value\",");
        assert!(t.contains(&("\"name\"".into(), TokenKind::Keyword)));
        assert!(t.contains(&("\"value\"".into(), TokenKind::Str)));
    }

    #[test]
    fn json_literals_and_numbers() {
        let line = "{\"a\": true, \"b\": null, \"c\": 1.5e3}";
        assert_eq!(of_kind("json", line, TokenKind::Number), ["1.5e3"]);
        let kws = of_kind("json", line, TokenKind::Keyword);
        assert!(kws.contains(&"true".to_string()));
        assert!(kws.contains(&"null".to_string()));
    }

    #[test]
    fn colon_inside_a_quoted_key_does_not_terminate_it() {
        let t = toks("json", "\"a:b\": 1");
        assert!(t.contains(&("\"a:b\"".into(), TokenKind::Keyword)));
        assert!(t.contains(&("1".into(), TokenKind::Number)));
    }

    #[test]
    fn yaml_hyphenated_key_and_list_item_key() {
        assert!(toks("yaml", "key-name: v").contains(&("key-name".into(), TokenKind::Keyword)));
        // The list marker is punctuation; the key starts after it.
        let t = toks("yaml", "- name: v");
        assert!(t.contains(&("name".into(), TokenKind::Keyword)));
        assert!(
            t.iter()
                .any(|(s, k)| s.starts_with('-') && *k == TokenKind::Text)
        );
    }

    #[test]
    fn yaml_url_value_keeps_only_the_first_colon_as_key_terminator() {
        let t = toks("yaml", "url: http://example.com");
        assert!(t.contains(&("url".into(), TokenKind::Keyword)));
        assert_eq!(
            of_kind("yaml", "url: http://example.com", TokenKind::Comment).len(),
            0
        );
    }

    #[test]
    fn yaml_comment_line_yields_no_key() {
        let line = "# note: not a key";
        assert_eq!(of_kind("yaml", line, TokenKind::Comment), [line]);
        assert!(of_kind("yaml", line, TokenKind::Keyword).is_empty());
    }

    #[test]
    fn toml_uses_equals_as_the_key_terminator() {
        assert!(toks("toml", "a.b.c = 1").contains(&("a.b.c".into(), TokenKind::Keyword)));
        // A section header has no terminator, so nothing is retagged.
        assert!(of_kind("toml", "[section]", TokenKind::Keyword).is_empty());
    }

    #[test]
    fn key_is_not_retagged_when_the_line_resumes_an_open_construct() {
        let spec = spec_for("toml").unwrap();
        let mut st = State::Triple("\"\"\"");
        let out = tokenize("still: inside = the string", spec, &mut st);
        assert_eq!(out, vec![(0..26, TokenKind::Str)]);
    }

    #[test]
    fn sh_keywords_and_comment() {
        let line = "for f in *.txt; do echo \"$f\"; done # loop";
        let kws = of_kind("sh", line, TokenKind::Keyword);
        assert!(kws.contains(&"for".to_string()));
        assert!(kws.contains(&"do".to_string()));
        assert!(kws.contains(&"done".to_string()));
        assert_eq!(of_kind("sh", line, TokenKind::Comment), ["# loop"]);
    }

    #[test]
    fn go_backtick_string_is_raw() {
        assert_eq!(
            of_kind("go", "s := `a\\b` + \"c\"", TokenKind::Str),
            ["`a\\b`", "\"c\""]
        );
    }

    #[test]
    fn jsts_shares_one_spec_with_the_union_keyword_list() {
        assert!(of_kind("js", "const x = 1;", TokenKind::Keyword).contains(&"const".to_string()));
        assert!(
            of_kind("ts", "interface A { b: string }", TokenKind::Keyword)
                .contains(&"interface".to_string())
        );
        assert!(std::ptr::eq(
            spec_for("js").unwrap(),
            spec_for("ts").unwrap()
        ));
    }

    #[test]
    fn number_forms() {
        let n = |s: &str| of_kind("rust", s, TokenKind::Number);
        assert_eq!(n("let a = 0xFF;"), ["0xFF"]);
        assert_eq!(n("let a = 0b1010;"), ["0b1010"]);
        assert_eq!(n("let a = 10_000;"), ["10_000"]);
        assert_eq!(n("let a = 1.5;"), ["1.5"]);
        assert_eq!(n("let a = 2e-3;"), ["2e-3"]);
        // A trailing `.` is a method call or range, not part of the number.
        assert_eq!(n("for i in 0..3 {}"), ["0", "3"]);
    }

    #[test]
    fn digits_inside_an_identifier_are_not_a_number() {
        assert!(of_kind("rust", "let x1 = y2;", TokenKind::Number).is_empty());
    }

    #[test]
    fn info_string_resolution_and_aliases() {
        for (info, expect_some) in [
            ("rust", true),
            ("Rust", true),
            ("rust,ignore", true),
            ("js {1,3}", true),
            ("yml", true),
            ("bash", true),
            ("golang", true),
            ("TypeScript", true),
            ("console", false),
            ("shell-session", false),
            ("mermaid", false),
            ("", false),
        ] {
            assert_eq!(
                spec_for(info).is_some(),
                expect_some,
                "info string {info:?}"
            );
        }
        assert!(std::ptr::eq(
            spec_for("yml").unwrap(),
            spec_for("yaml").unwrap()
        ));
        assert!(std::ptr::eq(
            spec_for("rs").unwrap(),
            spec_for("rust").unwrap()
        ));
    }

    #[test]
    fn multibyte_content_does_not_split_a_char() {
        // Byte ranges must land on char boundaries or the slicing in fence.rs
        // would panic.
        for line in ["let s = \"héllo → wörld\"; // ✓", "# 日本語のコメント"] {
            for lang in ["rust", "yaml"] {
                let spec = spec_for(lang).unwrap();
                let mut st = State::Normal;
                for (r, _) in tokenize(line, spec, &mut st) {
                    assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
                }
            }
        }
    }
}
