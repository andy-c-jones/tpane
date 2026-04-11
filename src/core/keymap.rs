use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::core::commands::Command;

/// A normalized key chord, e.g. Ctrl+Shift+T.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

impl KeyChord {
    pub fn from_event(event: &KeyEvent) -> Self {
        // Normalize Char to lowercase so bindings stored as "ctrl+shift+t"
        // match real events where crossterm reports Char('T') + SHIFT.
        let code = match event.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        Self { modifiers: event.modifiers, code }
    }

    /// Parse a human-readable chord string from Lua config.
    /// Format: optional modifiers separated by `+`, then a key name.
    /// E.g. "ctrl+shift+t", "ctrl+shift+w", "alt+f4"
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<String> = s.to_lowercase().split('+').map(str::to_string).collect();
        if parts.is_empty() {
            return None;
        }
        let key_name = parts.last()?;
        let mod_parts = &parts[..parts.len() - 1];

        let mut modifiers = KeyModifiers::empty();
        for m in mod_parts {
            match m.as_str() {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "alt" | "meta" => modifiers |= KeyModifiers::ALT,
                _ => return None,
            }
        }

        let code = parse_key_code(key_name.as_str())?;
        Some(Self { modifiers, code })
    }
}

fn parse_key_code(s: &str) -> Option<KeyCode> {
    match s {
        "enter" | "return" => Some(KeyCode::Enter),
        "space" => Some(KeyCode::Char(' ')),
        "tab" => Some(KeyCode::Tab),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "escape" | "esc" => Some(KeyCode::Esc),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => Some(KeyCode::PageDown),
        s if s.len() == 1 => {
            let ch = s.chars().next()?;
            Some(KeyCode::Char(ch))
        }
        s if s.starts_with('f') => {
            let n: u8 = s[1..].parse().ok()?;
            Some(KeyCode::F(n))
        }
        _ => None,
    }
}

/// Maps key chords to Commands.
pub struct KeyMap {
    bindings: HashMap<KeyChord, Command>,
}

impl KeyMap {
    pub fn new() -> Self {
        Self { bindings: HashMap::new() }
    }

    pub fn bind(&mut self, chord: KeyChord, command: Command) {
        self.bindings.insert(chord, command);
    }

    pub fn lookup(&self, event: &KeyEvent) -> Option<&Command> {
        let chord = KeyChord::from_event(event);
        self.bindings.get(&chord)
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        let mut km = KeyMap::new();
        // These defaults are overridable via main.lua.
        let defaults: &[(&str, Command)] = &[
            ("ctrl+shift+t", Command::SplitVertical),
            ("ctrl+shift+v", Command::SplitVertical),
            ("ctrl+shift+h", Command::SplitHorizontal),
            ("ctrl+shift+w", Command::ClosePane),
            ("ctrl+shift+n", Command::FocusNext),
            ("ctrl+shift+p", Command::FocusPrev),
            ("ctrl+shift+q", Command::Quit),
        ];
        for (chord_str, cmd) in defaults {
            if let Some(chord) = KeyChord::parse(chord_str) {
                km.bind(chord, cmd.clone());
            }
        }
        km
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── KeyChord::parse ───────────────────────────────────────────────────────

    #[test]
    fn parse_ctrl_shift_t() {
        let chord = KeyChord::parse("ctrl+shift+t").unwrap();
        assert!(chord.modifiers.contains(KeyModifiers::CONTROL));
        assert!(chord.modifiers.contains(KeyModifiers::SHIFT));
        assert_eq!(chord.code, KeyCode::Char('t'));
    }

    #[test]
    fn parse_uppercase_input_normalized() {
        // Chord strings should be case-insensitive
        let chord = KeyChord::parse("CTRL+SHIFT+T").unwrap();
        assert!(chord.modifiers.contains(KeyModifiers::CONTROL));
        assert_eq!(chord.code, KeyCode::Char('t'));
    }

    #[test]
    fn parse_alt_modifier() {
        let chord = KeyChord::parse("alt+f4").unwrap();
        assert!(chord.modifiers.contains(KeyModifiers::ALT));
        assert_eq!(chord.code, KeyCode::F(4));
    }

    #[test]
    fn parse_meta_alias() {
        let chord = KeyChord::parse("meta+x").unwrap();
        assert!(chord.modifiers.contains(KeyModifiers::ALT));
        assert_eq!(chord.code, KeyCode::Char('x'));
    }

    #[test]
    fn parse_control_alias() {
        let chord = KeyChord::parse("control+c").unwrap();
        assert!(chord.modifiers.contains(KeyModifiers::CONTROL));
        assert_eq!(chord.code, KeyCode::Char('c'));
    }

    #[test]
    fn parse_function_key() {
        let chord = KeyChord::parse("ctrl+f5").unwrap();
        assert_eq!(chord.code, KeyCode::F(5));
    }

    #[test]
    fn parse_all_function_keys_f1_f12() {
        for n in 1u8..=12 {
            let s = format!("f{}", n);
            let chord = KeyChord::parse(&s).unwrap();
            assert_eq!(chord.code, KeyCode::F(n), "f{n} failed");
        }
    }

    #[test]
    fn parse_special_keys() {
        let cases = [
            ("enter",     KeyCode::Enter),
            ("return",    KeyCode::Enter),
            ("space",     KeyCode::Char(' ')),
            ("tab",       KeyCode::Tab),
            ("backspace", KeyCode::Backspace),
            ("bs",        KeyCode::Backspace),
            ("delete",    KeyCode::Delete),
            ("del",       KeyCode::Delete),
            ("escape",    KeyCode::Esc),
            ("esc",       KeyCode::Esc),
            ("up",        KeyCode::Up),
            ("down",      KeyCode::Down),
            ("left",      KeyCode::Left),
            ("right",     KeyCode::Right),
            ("home",      KeyCode::Home),
            ("end",       KeyCode::End),
            ("pageup",    KeyCode::PageUp),
            ("pgup",      KeyCode::PageUp),
            ("pagedown",  KeyCode::PageDown),
            ("pgdn",      KeyCode::PageDown),
            ("pgdown",    KeyCode::PageDown),
        ];
        for (input, expected_code) in cases {
            let chord = KeyChord::parse(input).unwrap_or_else(|| panic!("failed to parse '{input}'"));
            assert_eq!(chord.code, expected_code);
        }
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(KeyChord::parse("ctrl+shift+blorp").is_none());
        assert!(KeyChord::parse("badmodifier+t").is_none());
    }

    #[test]
    fn parse_empty_string_returns_none() {
        assert!(KeyChord::parse("").is_none());
    }

    #[test]
    fn parse_no_modifier() {
        let chord = KeyChord::parse("enter").unwrap();
        assert!(chord.modifiers.is_empty());
        assert_eq!(chord.code, KeyCode::Enter);
    }

    // ── KeyChord::from_event (char normalization) ─────────────────────────────

    #[test]
    fn from_event_normalizes_uppercase_char() {
        // crossterm may send Char('T') + SHIFT for Shift+T
        let event = KeyEvent::new(KeyCode::Char('T'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        let chord = KeyChord::from_event(&event);
        assert_eq!(chord.code, KeyCode::Char('t'));
        assert!(chord.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn from_event_non_char_unchanged() {
        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        let chord = KeyChord::from_event(&event);
        assert_eq!(chord.code, KeyCode::Enter);
    }

    // ── KeyMap::lookup ────────────────────────────────────────────────────────

    #[test]
    fn lookup_hit_with_lowercase_event() {
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(km.lookup(&event), Some(&Command::SplitVertical));
    }

    #[test]
    fn lookup_hit_with_uppercase_event() {
        // Shift+T sends Char('T') in some terminals; should still match binding
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('T'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(km.lookup(&event), Some(&Command::SplitVertical));
    }

    #[test]
    fn lookup_all_default_bindings() {
        let km = KeyMap::default();
        let cases: &[(KeyCode, KeyModifiers, Command)] = &[
            (KeyCode::Char('t'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::SplitVertical),
            (KeyCode::Char('v'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::SplitVertical),
            (KeyCode::Char('h'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::SplitHorizontal),
            (KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::ClosePane),
            (KeyCode::Char('n'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::FocusNext),
            (KeyCode::Char('p'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::FocusPrev),
            (KeyCode::Char('q'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, Command::Quit),
        ];
        for (code, mods, expected) in cases {
            let event = KeyEvent::new(*code, *mods);
            assert_eq!(km.lookup(&event), Some(expected), "failed for {code:?}+{mods:?}");
        }
    }

    #[test]
    fn lookup_miss() {
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::empty());
        assert!(km.lookup(&event).is_none());
    }

    #[test]
    fn custom_bind_overrides_default() {
        let mut km = KeyMap::default();
        let chord = KeyChord::parse("ctrl+shift+t").unwrap();
        km.bind(chord, Command::Quit);
        let event = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(km.lookup(&event), Some(&Command::Quit));
    }
}
