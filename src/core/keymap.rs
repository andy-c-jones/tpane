//! Key chord parsing and key-to-command mapping.

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
    /// Convert a raw key event into a normalized chord key suitable for hashmap lookup.
    pub fn from_event(event: &KeyEvent) -> Self {
        // Normalize Char to lowercase so bindings stored as "ctrl+shift+t"
        // match real events where crossterm reports Char('T') + SHIFT.
        let code = match event.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        Self {
            modifiers: event.modifiers,
            code,
        }
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

/// Maps key chords to Commands, with prefix key support.
pub struct KeyMap {
    /// The prefix key (e.g. Ctrl+B). When pressed, the next key is looked up
    /// in `prefix_bindings` instead of being forwarded to the active pane.
    pub prefix_key: KeyChord,
    /// Bindings that activate after the prefix key.
    prefix_bindings: HashMap<KeyChord, Command>,
    /// Bindings that fire directly (without the prefix key).
    /// These are checked before forwarding input to the active pane,
    /// so they can be held down for continuous repeated actions (e.g. resize).
    direct_bindings: HashMap<KeyChord, Command>,
}

impl KeyMap {
    /// Create an empty key map with the default prefix key (`Ctrl+B`).
    pub fn new() -> Self {
        Self {
            prefix_key: KeyChord::parse("ctrl+b").unwrap(),
            prefix_bindings: HashMap::new(),
            direct_bindings: HashMap::new(),
        }
    }

    /// Register a prefix binding triggered after the prefix key is active.
    pub fn bind(&mut self, chord: KeyChord, command: Command) {
        self.prefix_bindings.insert(chord, command);
    }

    /// Register a direct (non-prefix) binding.
    pub fn bind_direct(&mut self, chord: KeyChord, command: Command) {
        self.direct_bindings.insert(chord, command);
    }

    /// Check if a key event matches the prefix key.
    pub fn is_prefix(&self, event: &KeyEvent) -> bool {
        KeyChord::from_event(event) == self.prefix_key
    }

    /// Look up a command in the prefix bindings (called after prefix key).
    pub fn lookup_prefix(&self, event: &KeyEvent) -> Option<&Command> {
        let chord = KeyChord::from_event(event);
        self.prefix_bindings.get(&chord)
    }

    /// Look up a command in the direct bindings (checked on every key event).
    pub fn lookup_direct(&self, event: &KeyEvent) -> Option<&Command> {
        let chord = KeyChord::from_event(event);
        self.direct_bindings.get(&chord)
    }

    /// Return all prefix chords currently mapped to `command`.
    pub fn prefix_chords_for_command(&self, command: Command) -> Vec<KeyChord> {
        self.prefix_bindings
            .iter()
            .filter_map(|(chord, cmd)| {
                if *cmd == command {
                    Some(chord.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Return all direct chords currently mapped to `command`.
    pub fn direct_chords_for_command(&self, command: Command) -> Vec<KeyChord> {
        self.direct_bindings
            .iter()
            .filter_map(|(chord, cmd)| {
                if *cmd == command {
                    Some(chord.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Legacy lookup that checks prefix bindings directly (for tests).
    #[allow(dead_code)]
    pub fn lookup(&self, event: &KeyEvent) -> Option<&Command> {
        self.lookup_prefix(event)
    }
}

impl Default for KeyMap {
    fn default() -> Self {
        let mut km = KeyMap::new();
        let defaults: &[(&str, Command)] = &[
            // Directional splits: Ctrl+Arrow after prefix
            ("ctrl+left", Command::SplitLeft),
            ("ctrl+right", Command::SplitRight),
            ("ctrl+up", Command::SplitUp),
            ("ctrl+down", Command::SplitDown),
            // Focus movement: Arrow after prefix (spatial navigation)
            ("left", Command::FocusLeft),
            ("right", Command::FocusRight),
            ("up", Command::FocusUp),
            ("down", Command::FocusDown),
            // Other commands
            ("w", Command::ClosePane),
            ("q", Command::Quit),
        ];
        for (chord_str, cmd) in defaults {
            if let Some(chord) = KeyChord::parse(chord_str) {
                km.bind(chord, cmd.clone());
            }
        }

        // Direct resize bindings: Alt+Shift+Arrow (no prefix needed; holdable).
        let direct_defaults: &[(&str, Command)] = &[
            ("alt+shift+left", Command::ResizeLeft),
            ("alt+shift+right", Command::ResizeRight),
            ("alt+shift+up", Command::ResizeUp),
            ("alt+shift+down", Command::ResizeDown),
        ];
        for (chord_str, cmd) in direct_defaults {
            if let Some(chord) = KeyChord::parse(chord_str) {
                km.bind_direct(chord, cmd.clone());
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
            ("enter", KeyCode::Enter),
            ("return", KeyCode::Enter),
            ("space", KeyCode::Char(' ')),
            ("tab", KeyCode::Tab),
            ("backspace", KeyCode::Backspace),
            ("bs", KeyCode::Backspace),
            ("delete", KeyCode::Delete),
            ("del", KeyCode::Delete),
            ("escape", KeyCode::Esc),
            ("esc", KeyCode::Esc),
            ("up", KeyCode::Up),
            ("down", KeyCode::Down),
            ("left", KeyCode::Left),
            ("right", KeyCode::Right),
            ("home", KeyCode::Home),
            ("end", KeyCode::End),
            ("pageup", KeyCode::PageUp),
            ("pgup", KeyCode::PageUp),
            ("pagedown", KeyCode::PageDown),
            ("pgdn", KeyCode::PageDown),
            ("pgdown", KeyCode::PageDown),
        ];
        for (input, expected_code) in cases {
            let chord =
                KeyChord::parse(input).unwrap_or_else(|| panic!("failed to parse '{input}'"));
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
        let event = KeyEvent::new(
            KeyCode::Char('T'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
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

    // ── KeyMap prefix and lookup ────────────────────────────────────────────

    #[test]
    fn prefix_key_is_ctrl_b_by_default() {
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert!(km.is_prefix(&event));
    }

    #[test]
    fn non_prefix_key_is_not_prefix() {
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert!(!km.is_prefix(&event));
    }

    #[test]
    fn lookup_all_default_prefix_bindings() {
        let km = KeyMap::default();
        let cases: &[(KeyCode, KeyModifiers, Command)] = &[
            (KeyCode::Left, KeyModifiers::CONTROL, Command::SplitLeft),
            (KeyCode::Right, KeyModifiers::CONTROL, Command::SplitRight),
            (KeyCode::Up, KeyModifiers::CONTROL, Command::SplitUp),
            (KeyCode::Down, KeyModifiers::CONTROL, Command::SplitDown),
            (KeyCode::Left, KeyModifiers::empty(), Command::FocusLeft),
            (KeyCode::Right, KeyModifiers::empty(), Command::FocusRight),
            (KeyCode::Up, KeyModifiers::empty(), Command::FocusUp),
            (KeyCode::Down, KeyModifiers::empty(), Command::FocusDown),
            (
                KeyCode::Char('w'),
                KeyModifiers::empty(),
                Command::ClosePane,
            ),
            (KeyCode::Char('q'), KeyModifiers::empty(), Command::Quit),
        ];
        for (code, mods, expected) in cases {
            let event = KeyEvent::new(*code, *mods);
            assert_eq!(
                km.lookup_prefix(&event),
                Some(expected),
                "failed for {code:?}+{mods:?}"
            );
        }
    }

    #[test]
    fn lookup_miss() {
        let km = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::empty());
        assert!(km.lookup_prefix(&event).is_none());
    }

    #[test]
    fn custom_bind_overrides_default() {
        let mut km = KeyMap::default();
        let chord = KeyChord::parse("w").unwrap();
        km.bind(chord, Command::Quit);
        let event = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty());
        assert_eq!(km.lookup_prefix(&event), Some(&Command::Quit));
    }

    // ── direct bindings ──────────────────────────────────────────────────────

    #[test]
    fn default_direct_bindings_include_resize() {
        let km = KeyMap::default();
        let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
        let cases: &[(KeyCode, Command)] = &[
            (KeyCode::Left, Command::ResizeLeft),
            (KeyCode::Right, Command::ResizeRight),
            (KeyCode::Up, Command::ResizeUp),
            (KeyCode::Down, Command::ResizeDown),
        ];
        for (code, expected) in cases {
            let event = KeyEvent::new(*code, alt_shift);
            assert_eq!(
                km.lookup_direct(&event),
                Some(expected),
                "missing direct binding for {code:?}"
            );
        }
    }

    #[test]
    fn direct_binding_does_not_appear_in_prefix_bindings() {
        let km = KeyMap::default();
        let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
        let event = KeyEvent::new(KeyCode::Left, alt_shift);
        assert!(km.lookup_prefix(&event).is_none());
    }

    #[test]
    fn bind_direct_adds_custom_direct_binding() {
        let mut km = KeyMap::new();
        let chord = KeyChord::parse("alt+r").unwrap();
        km.bind_direct(chord, Command::ResizeRight);
        let event = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT);
        assert_eq!(km.lookup_direct(&event), Some(&Command::ResizeRight));
    }

    #[test]
    fn prefix_chords_for_command_returns_all_matches() {
        let mut km = KeyMap::new();
        km.bind(KeyChord::parse("w").unwrap(), Command::ClosePane);
        km.bind(KeyChord::parse("x").unwrap(), Command::ClosePane);
        let mut chords = km.prefix_chords_for_command(Command::ClosePane);
        chords.sort_by_key(|ch| format!("{:?}+{:?}", ch.modifiers, ch.code));
        assert_eq!(chords.len(), 2);
    }

    #[test]
    fn direct_chords_for_command_returns_all_matches() {
        let mut km = KeyMap::new();
        km.bind_direct(KeyChord::parse("alt+h").unwrap(), Command::ResizeLeft);
        km.bind_direct(KeyChord::parse("alt+left").unwrap(), Command::ResizeLeft);
        let mut chords = km.direct_chords_for_command(Command::ResizeLeft);
        chords.sort_by_key(|ch| format!("{:?}+{:?}", ch.modifiers, ch.code));
        assert_eq!(chords.len(), 2);
    }
}
