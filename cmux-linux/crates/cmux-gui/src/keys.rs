//! Translating keys to the bytes a PTY expects.
//!
//! Two entrypoints share the logic: [`chord_to_bytes`] parses textual chords
//! from the `cmux send-key` CLI (`"enter"`, `"ctrl+c"`, `"up"`), and
//! [`key_event_to_bytes`] maps a live Dioxus keyboard event to bytes.

use dioxus::events::{Key, Modifiers};

/// Parse a textual chord like `"ctrl+c"`, `"enter"`, `"up"`, or a single
/// character into the bytes to write to a PTY. Returns `None` if unrecognized.
pub fn chord_to_bytes(chord: &str) -> Option<Vec<u8>> {
    let chord = chord.trim();
    if chord.is_empty() {
        return None;
    }
    let parts: Vec<&str> = chord.split('+').map(|s| s.trim()).collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let key = key[0];
    let ctrl = mods.iter().any(|m| matches!(*m, "ctrl" | "control" | "c"));
    let alt = mods.iter().any(|m| matches!(*m, "alt" | "meta" | "option"));

    let base: Vec<u8> = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" | "cr" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "esc" | "escape" => vec![0x1b],
        "space" => vec![b' '],
        "backspace" | "bs" => vec![0x7f],
        "delete" | "del" => vec![0x1b, b'[', b'3', b'~'],
        "up" => vec![0x1b, b'[', b'A'],
        "down" => vec![0x1b, b'[', b'B'],
        "right" => vec![0x1b, b'[', b'C'],
        "left" => vec![0x1b, b'[', b'D'],
        "home" => vec![0x1b, b'[', b'H'],
        "end" => vec![0x1b, b'[', b'F'],
        "pageup" => vec![0x1b, b'[', b'5', b'~'],
        "pagedown" => vec![0x1b, b'[', b'6', b'~'],
        other => {
            // Single printable character.
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    if ctrl {
                        return Some(vec![control_byte(c)?]);
                    }
                    let mut b = String::new();
                    b.push(c);
                    b.into_bytes()
                }
                _ => return None,
            }
        }
    };

    if alt {
        // Alt-prefixed: ESC then the key bytes.
        let mut out = vec![0x1b];
        out.extend(base);
        Some(out)
    } else {
        Some(base)
    }
}

/// Map a control character: ctrl+a → 0x01 … ctrl+z → 0x1a, plus a few symbols.
fn control_byte(c: char) -> Option<u8> {
    let u = c.to_ascii_uppercase();
    match u {
        'A'..='Z' => Some((u as u8) - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        ' ' | '@' => Some(0x00),
        _ => None,
    }
}

/// Translate a live keyboard event into PTY bytes.
pub fn key_event_to_bytes(key: &Key, mods: Modifiers) -> Option<Vec<u8>> {
    let ctrl = mods.ctrl();
    let alt = mods.alt() || mods.meta();

    let base: Vec<u8> = match key {
        Key::Character(s) => {
            if ctrl {
                let c = s.chars().next()?;
                return Some(vec![control_byte(c)?]);
            }
            s.clone().into_bytes()
        }
        Key::Enter => vec![b'\r'],
        Key::Tab => vec![b'\t'],
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::Delete => vec![0x1b, b'[', b'3', b'~'],
        Key::ArrowUp => vec![0x1b, b'[', b'A'],
        Key::ArrowDown => vec![0x1b, b'[', b'B'],
        Key::ArrowRight => vec![0x1b, b'[', b'C'],
        Key::ArrowLeft => vec![0x1b, b'[', b'D'],
        Key::Home => vec![0x1b, b'[', b'H'],
        Key::End => vec![0x1b, b'[', b'F'],
        Key::PageUp => vec![0x1b, b'[', b'5', b'~'],
        Key::PageDown => vec![0x1b, b'[', b'6', b'~'],
        _ => return None,
    };

    if alt && !matches!(key, Key::Character(_)) {
        let mut out = vec![0x1b];
        out.extend(base);
        Some(out)
    } else if alt {
        let mut out = vec![0x1b];
        out.extend(base);
        Some(out)
    } else {
        Some(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_keys() {
        assert_eq!(chord_to_bytes("enter"), Some(vec![b'\r']));
        assert_eq!(chord_to_bytes("tab"), Some(vec![b'\t']));
        assert_eq!(chord_to_bytes("esc"), Some(vec![0x1b]));
        assert_eq!(chord_to_bytes("backspace"), Some(vec![0x7f]));
    }

    #[test]
    fn arrows() {
        assert_eq!(chord_to_bytes("up"), Some(vec![0x1b, b'[', b'A']));
        assert_eq!(chord_to_bytes("left"), Some(vec![0x1b, b'[', b'D']));
    }

    #[test]
    fn ctrl_letters() {
        assert_eq!(chord_to_bytes("ctrl+c"), Some(vec![0x03]));
        assert_eq!(chord_to_bytes("ctrl+a"), Some(vec![0x01]));
        assert_eq!(chord_to_bytes("ctrl+d"), Some(vec![0x04]));
    }

    #[test]
    fn single_char() {
        assert_eq!(chord_to_bytes("x"), Some(vec![b'x']));
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(chord_to_bytes("alt+b"), Some(vec![0x1b, b'b']));
    }

    #[test]
    fn unknown_is_none() {
        assert_eq!(chord_to_bytes("frobnicate"), None);
        assert_eq!(chord_to_bytes(""), None);
    }

    #[test]
    fn event_character_and_ctrl() {
        let k = Key::Character("a".into());
        assert_eq!(key_event_to_bytes(&k, Modifiers::empty()), Some(vec![b'a']));
        assert_eq!(key_event_to_bytes(&k, Modifiers::CONTROL), Some(vec![0x01]));
    }

    #[test]
    fn event_enter_and_arrows() {
        assert_eq!(
            key_event_to_bytes(&Key::Enter, Modifiers::empty()),
            Some(vec![b'\r'])
        );
        assert_eq!(
            key_event_to_bytes(&Key::ArrowUp, Modifiers::empty()),
            Some(vec![0x1b, b'[', b'A'])
        );
    }
}
