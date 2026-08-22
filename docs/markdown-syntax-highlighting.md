# Fenced code syntax highlighting (#122, 2026-08-22)

Ticket: pgmac-net/gh-issues-tui#122

## What changed

Fenced code blocks — which #120 gave a gutter bar, a filled background and a
right-aligned language tag — now colour their contents per language:

```
▏ fn total(items: &[Item], discount: f64) -> f64 {                      rust
▏     let label = "line items"; // trailing comment
▏     items.iter().map(|i| i.price).sum::<f64>() * (1.0 - discount)
▏ }
```

(`fn`/`let` in keyword colour, `"line items"` in string colour, `// trailing
comment` in comment colour, `1.0` in number colour — not visible in plain text
above.)

Eight specs ship: `sh`, `yaml`, `json`, `toml`, `rust`, `python`, `go`, and a
shared `js`/`ts`. Anything else renders exactly as it did before #122.

## Decisions

Nine decisions, all taken on the ticket before implementation:

| # | Decision | Chosen | Why |
|---|---|---|---|
| 1 | Tokenizer architecture | One generic scanner + per-language `LangSpec` tables + structural flags | Table-driven leaves one scanner to fix rather than eight; the structural flag stops `json`/`yaml`/`toml` rendering as one flat colour, which a pure keyword table would do |
| 2 | Theme surface | 4 new fields: `code_keyword`, `code_string`, `code_comment`, `code_number` | Matches the ticket's token list. The structural *key* token borrows `code_keyword` rather than earning a fifth key |
| 3 | Cross-line state | `Normal \| BlockComment \| Triple` | Multi-line `/* */` and `"""` are common in the samples this feature exists to render. Everything else degrades to plain text |
| 4 | Order vs `hard_break` | Tokenize the whole source line, *then* break span-aware | Colours must be identical at every pane width. Breaking first would recolour code as the user resizes the pane |
| 5 | Info string → spec | First word, lowercased, through an alias map | Handles `Rust`, `rust,ignore`, `js {1,3}`, `yml`, `bash`. `console`/`shell-session` deliberately excluded |
| 6 | Language set | All 8 in one change; `js`+`ts` share one spec | The tables are the cheap part once the scanner exists. One duplicated 40-word keyword list to drift is worse than `interface` colouring inside a `.js` block |
| 7 | Off switch | None — point the 4 token colours at `code_fg` | No second way to express one outcome; matches #120's theme-only precedent |
| 8 | Verification | Unit tests *and* a live pty/pyte capture | A span assertion cannot tell you `code_comment` is unreadable on `code_bg` |
| 9 | Default colours | Explicit RGB, One Dark derived | #120 decision 3: explicit RGB is self-consistent on any terminal background |

## Approach

`src/tui/markdown/highlight.rs` is one forward scanner over a source line,
driven by a `LangSpec` const table per language:

```rust
pub(super) struct LangSpec {
    keywords:        &'static [&'static str],
    line_comment:    &'static [&'static str],
    block_comment:   Option<(&'static str, &'static str)>,
    triple:          &'static [&'static str],
    strings:         &'static [StringRule],
    key_terminators: &'static [char],
}
```

Precedence per position: resume an open construct → line comment → block
comment → triple quote → string → number → identifier/keyword → plain text.
Identifiers are consumed whole before the keyword lookup, so `iffy` never
matches `if`. The returned byte ranges are non-empty, ascending, adjacent and
cover the whole line — `fence.rs` relies on that to build spans without gaps.

**Tokenising happens before the pane-width break.** `fence::hard_break` now
takes a list of `(String, Style)` segments and returns `Vec<Vec<Span>>`,
splitting a segment where the width boundary lands inside it. The
grapheme-and-width logic from #120 is unchanged; it just carries a style
alongside. This is what makes `colours_do_not_change_with_pane_width` hold — a
token split across two rows keeps one colour, and resizing recolours nothing.

**The structural key rule.** `json`, `yaml` and `toml` have almost no keywords,
so a keyword table alone would render them flat. `retag_key` takes the line's
first non-space run (stepping over a YAML `- ` list marker), scans for the
spec's terminator (`:` or `=`) while stepping *over* string tokens and stopping
at a comment, and retags that byte range as `Keyword`. Working on the range
rather than on whole tokens makes hyphenated (`app-name:`), dotted (`a.b.c =`)
and quoted (`"key":`) keys one rule. A `:` inside a quoted key doesn't
terminate it; a `# note: not a key` comment line yields no key at all.

**Cross-line state is per fence.** `render` creates one `State::Normal` and
threads it through the block's lines, so a `/* */` or `"""` spans rows
correctly and a following fence always starts clean.

**Highlighting keys off the info string, not off the drawn tag.** #120 drops
the language tag when the first row's code fills the width; that must not
silently disable colour, and `highlighting_survives_the_language_tag_being_dropped`
pins it.

### Theme

Four new optional profile keys, defaulting to explicit RGB chosen for contrast
against `code_bg`:

| Key | Default | Covers |
|---|---|---|
| `code_keyword` | `#c678dd` | keywords, and json/yaml/toml keys |
| `code_string` | `#98c379` | string literals |
| `code_comment` | `#7f848e` | comments |
| `code_number` | `#d19a66` | numeric literals |

Every token style keeps `code_bg`, so a highlighted row fills exactly as a flat
one does. There is no on/off switch — the README documents a `flat` profile
that sets all four to `code_fg`.

### Deliberately unhandled

Each of these degrades to plain `code_fg`, never to a colour that runs away
down the block:

- Heredocs (`<<EOF`) and backslash line continuations
- Template-literal interpolation and JSX
- Rust raw strings (`r#"…"#`)
- Rust char literals — `'` is *not* a Rust string delimiter here, deliberately:
  including it would let a lifetime (`'a`) open a string that never closes,
  which is a much worse failure than `'x'` rendering as plain text
- Numeric type suffixes — `1u32` is a number followed by an identifier
- Nested/second keys on one line — only the first key of a line is retagged,
  so `"nested": { "a:b": 1 }` colours `"nested"` as a key and `"a:b"` as a
  string
- Indented code blocks and inline `` `code` `` — both out of scope, as in #120

## Deviations from the plan

None. All nine decisions landed as agreed. One bug surfaced during testing and
was fixed rather than worked around: the key range initially swallowed the
whitespace before its terminator (`a.b.c = 1` retagged `"a.b.c "`), so
`retag_key` now trims the range end.

## Testing

- **26 unit tests in `markdown::highlight`**: range contiguity/coverage across
  10 mixed lines including empty and whitespace-only; per-language
  keyword/string/comment/number for rust, sh, go, python, json, yaml, toml,
  js/ts; keywords not matching inside longer identifiers; Rust lifetimes not
  opening a string; escaped quotes not closing one; shell/Go raw strings;
  unterminated strings stopping at EOL *without* carrying state; block comments
  across three lines and opened/closed on one; Python triple quotes both ways;
  json/yaml/toml key retagging including hyphenated, dotted, quoted, list-item,
  comment-line and section-header cases; no retag on a line that resumes an
  open construct; number forms (hex, binary, `_` separators, float, exponent,
  `0..3` not being one number); alias resolution including `console` → `None`;
  and byte ranges always landing on char boundaries for multibyte content.
- **9 new unit tests in `markdown::fence`**: the #120 exact-row-width invariant
  re-pinned with highlighting on at four widths; per-kind colours with
  `code_bg` preserved on every span; a token split by the hard-break keeping
  its colour on both rows; identical colours at width 80 and 24; unknown
  language and bare fences staying flat; highlighting surviving a dropped
  language tag; block-comment state carrying within a fence but not between
  fences; and the degenerate narrow pane staying flat.
- **2 new tests in `tui::theme`**: every default token colour differing from
  `code_fg`, profile overrides landing independently, and the documented flat
  recipe being expressible.
- `ui::detail::fence_measured_height_matches_the_rendered_rows` extended with a
  second body carrying a three-line block comment and a breaking string, so
  highlighting is pinned not to change row *counts* at three widths.
- Full suite **590 passed**. `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean.

### Live verification

Driven through the real TUI with the pty + pyte recipe in
`.claude/skills/verify/SKILL.md`, against a comment posted to #122 carrying all
eight languages, a three-line block comment, a Python docstring, an unknown
`mermaid` fence and a string long enough to split. Captured per-cell foreground
colours at 200 and 120 columns, under the `gruvbox` profile (which sets
`code_bg`/`code_fg` but not the new token keys, so the defaults were exercised
against a non-default background). Confirmed:

- Rust block comment held comment colour across all three rows; `0xFF`, `1.0`
  number-coloured; `fn`/`let`/`as` keyword-coloured
- Python docstring held string colour across its blank line and body
- Shell `'processing'` closed correctly as a raw string; `# single quotes are
  raw` comment-coloured
- YAML `apiVersion`, `app-name` (hyphenated) and `url` keyed correctly;
  `# a comment line: not a key` produced no key; `https://example.com/path`
  was not mistaken for a comment
- JSON keys vs value strings distinct; `1`, `2.5`, `1.5e3` numbers; `true`,
  `null` keywords
- TOML `raw = 'no \escapes here'` terminated at the closing quote;
  `[package]`/`[dependencies]` left plain
- `mermaid` block entirely flat `code_fg`
- The long Rust string split across two rows at 120 columns with both halves
  string-coloured, and `0xFF`/`as` keeping their kinds across a break that
  moved with the width
