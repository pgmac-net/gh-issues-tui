//! Encoding crossterm key events back into the bytes a terminal would have
//! sent (#23).
//!
//! Crossterm decodes the terminal's escape sequences into `KeyEvent`s; a
//! harness session needs them turned back into bytes for the child. That
//! round trip is this module's whole job, and it is pinned by tests because
//! a wrong sequence shows up as an agent that mysteriously ignores arrow
//! keys rather than as a crash.
//!
//! Sequences follow xterm, matching the `TERM=xterm-256color` the child is
//! started with.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Bytes to send to the child for `key`, or `None` for keys with no terminal
/// representation (bare modifier presses, media keys).
pub fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let m = key.modifiers;
    let alt = m.contains(KeyModifiers::ALT);

    let base: Vec<u8> = match key.code {
        KeyCode::Char(c) => return Some(encode_char(c, m)),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor(b'A', m),
        KeyCode::Down => cursor(b'B', m),
        KeyCode::Right => cursor(b'C', m),
        KeyCode::Left => cursor(b'D', m),
        KeyCode::Home => cursor(b'H', m),
        KeyCode::End => cursor(b'F', m),
        KeyCode::Insert => tilde(2, m),
        KeyCode::Delete => tilde(3, m),
        KeyCode::PageUp => tilde(5, m),
        KeyCode::PageDown => tilde(6, m),
        KeyCode::F(n) => function(n, m)?,
        _ => return None,
    };

    // Alt on a non-character key is an ESC prefix, except where the sequence
    // already encodes the modifier in its parameter.
    if alt && matches!(key.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace) {
        let mut out = vec![0x1b];
        out.extend_from_slice(&base);
        return Some(out);
    }
    Some(base)
}

/// Printable keys, plus the control and meta forms of them.
fn encode_char(c: char, m: KeyModifiers) -> Vec<u8> {
    let mut out = Vec::new();
    if m.contains(KeyModifiers::ALT) {
        out.push(0x1b);
    }
    if m.contains(KeyModifiers::CONTROL) {
        // Ctrl clears the top bits of the ASCII code: Ctrl+A is 0x01,
        // Ctrl+[ is ESC, Ctrl+? is DEL. Anything outside that range has no
        // control form and is sent as-is.
        let ctrl = match c {
            'a'..='z' => Some(c as u8 - b'a' + 1),
            'A'..='Z' => Some(c as u8 - b'A' + 1),
            '@' | ' ' => Some(0),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' | '-' => Some(0x1f),
            '?' => Some(0x7f),
            _ => None,
        };
        if let Some(byte) = ctrl {
            out.push(byte);
            return out;
        }
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    out
}

/// xterm's modifier parameter: 1 plus a bitmask of shift/alt/ctrl.
fn modifier_param(m: KeyModifiers) -> u8 {
    1 + u8::from(m.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(m.contains(KeyModifiers::ALT))
        + 4 * u8::from(m.contains(KeyModifiers::CONTROL))
}

/// Cursor and Home/End keys: `ESC [ A` unmodified, `ESC [ 1 ; m A` otherwise.
fn cursor(final_byte: u8, m: KeyModifiers) -> Vec<u8> {
    let param = modifier_param(m);
    if param == 1 {
        vec![0x1b, b'[', final_byte]
    } else {
        format!("\x1b[1;{param}{}", final_byte as char).into_bytes()
    }
}

/// Keypad-style keys: `ESC [ n ~`, or `ESC [ n ; m ~` when modified.
fn tilde(n: u8, m: KeyModifiers) -> Vec<u8> {
    let param = modifier_param(m);
    if param == 1 {
        format!("\x1b[{n}~").into_bytes()
    } else {
        format!("\x1b[{n};{param}~").into_bytes()
    }
}

/// Function keys. F1–F4 are SS3 sequences; F5 upwards are keypad-style, with
/// the well-known gaps at 16 and 22.
fn function(n: u8, m: KeyModifiers) -> Option<Vec<u8>> {
    let param = modifier_param(m);
    if (1..=4).contains(&n) {
        let final_byte = b'P' + (n - 1);
        return Some(if param == 1 {
            vec![0x1b, b'O', final_byte]
        } else {
            format!("\x1b[1;{param}{}", final_byte as char).into_bytes()
        });
    }
    let code = match n {
        5 => 15,
        6..=10 => 17 + (n - 6),
        11 => 23,
        12 => 24,
        _ => return None,
    };
    Some(tilde(code, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        encode(KeyEvent::new(code, m)).expect("encodable")
    }

    fn enc(code: KeyCode) -> Vec<u8> {
        encode(key(code)).expect("encodable")
    }

    #[test]
    fn plain_characters_are_their_utf8_bytes() {
        assert_eq!(enc(KeyCode::Char('a')), b"a");
        assert_eq!(enc(KeyCode::Char('Z')), b"Z");
        assert_eq!(enc(KeyCode::Char('é')), "é".as_bytes());
        assert_eq!(enc(KeyCode::Char('→')), "→".as_bytes());
    }

    #[test]
    fn control_characters_use_the_ascii_control_range() {
        assert_eq!(with(KeyCode::Char('a'), KeyModifiers::CONTROL), vec![0x01]);
        assert_eq!(with(KeyCode::Char('c'), KeyModifiers::CONTROL), vec![0x03]);
        // Ctrl+C must reach the agent — it is how you interrupt it.
        assert_eq!(with(KeyCode::Char('C'), KeyModifiers::CONTROL), vec![0x03]);
        assert_eq!(with(KeyCode::Char('['), KeyModifiers::CONTROL), vec![0x1b]);
        assert_eq!(with(KeyCode::Char(' '), KeyModifiers::CONTROL), vec![0x00]);
        assert_eq!(with(KeyCode::Char('?'), KeyModifiers::CONTROL), vec![0x7f]);
    }

    #[test]
    fn a_character_with_no_control_form_is_sent_as_itself() {
        assert_eq!(with(KeyCode::Char('7'), KeyModifiers::CONTROL), b"7");
    }

    #[test]
    fn alt_prefixes_a_character_with_escape() {
        assert_eq!(
            with(KeyCode::Char('b'), KeyModifiers::ALT),
            vec![0x1b, b'b']
        );
        let both = KeyModifiers::ALT | KeyModifiers::CONTROL;
        assert_eq!(with(KeyCode::Char('b'), both), vec![0x1b, 0x02]);
    }

    #[test]
    fn the_editing_keys_match_xterm() {
        assert_eq!(enc(KeyCode::Enter), b"\r");
        assert_eq!(enc(KeyCode::Tab), b"\t");
        assert_eq!(enc(KeyCode::BackTab), b"\x1b[Z");
        assert_eq!(enc(KeyCode::Backspace), vec![0x7f]);
        assert_eq!(enc(KeyCode::Esc), vec![0x1b]);
    }

    #[test]
    fn shift_tab_is_distinct_from_tab() {
        // Agents use Shift+Tab to cycle modes; collapsing it into Tab would
        // silently break that.
        assert_ne!(enc(KeyCode::Tab), enc(KeyCode::BackTab));
    }

    #[test]
    fn arrows_are_csi_sequences() {
        assert_eq!(enc(KeyCode::Up), b"\x1b[A");
        assert_eq!(enc(KeyCode::Down), b"\x1b[B");
        assert_eq!(enc(KeyCode::Right), b"\x1b[C");
        assert_eq!(enc(KeyCode::Left), b"\x1b[D");
        assert_eq!(enc(KeyCode::Home), b"\x1b[H");
        assert_eq!(enc(KeyCode::End), b"\x1b[F");
    }

    #[test]
    fn modified_arrows_carry_the_xterm_modifier_parameter() {
        assert_eq!(with(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(with(KeyCode::Right, KeyModifiers::SHIFT), b"\x1b[1;2C");
        assert_eq!(with(KeyCode::Up, KeyModifiers::ALT), b"\x1b[1;3A");
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(with(KeyCode::Down, ctrl_shift), b"\x1b[1;6B");
    }

    #[test]
    fn keypad_keys_use_tilde_sequences() {
        assert_eq!(enc(KeyCode::Insert), b"\x1b[2~");
        assert_eq!(enc(KeyCode::Delete), b"\x1b[3~");
        assert_eq!(enc(KeyCode::PageUp), b"\x1b[5~");
        assert_eq!(enc(KeyCode::PageDown), b"\x1b[6~");
        assert_eq!(with(KeyCode::Delete, KeyModifiers::CONTROL), b"\x1b[3;5~");
    }

    #[test]
    fn function_keys_cover_both_encodings_and_their_gaps() {
        assert_eq!(enc(KeyCode::F(1)), b"\x1bOP");
        assert_eq!(enc(KeyCode::F(4)), b"\x1bOS");
        assert_eq!(enc(KeyCode::F(5)), b"\x1b[15~");
        assert_eq!(enc(KeyCode::F(6)), b"\x1b[17~", "16 is skipped");
        assert_eq!(enc(KeyCode::F(10)), b"\x1b[21~");
        assert_eq!(enc(KeyCode::F(11)), b"\x1b[23~", "22 is skipped");
        assert_eq!(enc(KeyCode::F(12)), b"\x1b[24~");
        assert_eq!(encode(key(KeyCode::F(25))), None);
    }

    #[test]
    fn a_literal_f12_can_be_sent_through_the_chord() {
        // F12 F12 forwards the key the prefix would otherwise have eaten.
        assert_eq!(enc(KeyCode::F(12)), b"\x1b[24~");
    }

    #[test]
    fn keys_with_no_terminal_representation_encode_to_nothing() {
        assert_eq!(encode(key(KeyCode::CapsLock)), None);
        assert_eq!(encode(key(KeyCode::Menu)), None);
    }
}
