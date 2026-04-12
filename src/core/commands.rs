//! Command identifiers understood by the app dispatcher and Lua config parser.
//!
//! # Notes
//!
//! These values are shared by keymap bindings, Lua configuration parsing, and
//! runtime dispatch in [`crate::app::App`].

/// All actions that can be dispatched in tpane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SplitVertical,
    SplitHorizontal,
    /// Split vertical, new pane on left, focus moves to it.
    SplitLeft,
    /// Split vertical, new pane on right, focus moves to it.
    SplitRight,
    /// Split horizontal, new pane above, focus moves to it.
    SplitUp,
    /// Split horizontal, new pane below, focus moves to it.
    SplitDown,
    ClosePane,
    FocusNext,
    FocusPrev,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    /// Grow the active pane to the left (move its left edge leftward).
    ResizeLeft,
    /// Grow the active pane to the right (move its right edge rightward).
    ResizeRight,
    /// Grow the active pane upward (move its top edge upward).
    ResizeUp,
    /// Grow the active pane downward (move its bottom edge downward).
    ResizeDown,
    Quit,
}

impl Command {
    /// Parse a command name string from Lua config into a [`Command`].
    ///
    /// # Examples
    ///
    /// ```text
    /// split_right -> Command::SplitRight
    /// close       -> Command::ClosePane
    /// ```
    ///
    /// # Behavior
    ///
    /// Unknown or unsupported command names return `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "split_vertical" => Some(Self::SplitVertical),
            "split_horizontal" => Some(Self::SplitHorizontal),
            "split" => Some(Self::SplitVertical), // alias
            "split_left" => Some(Self::SplitLeft),
            "split_right" => Some(Self::SplitRight),
            "split_up" => Some(Self::SplitUp),
            "split_down" => Some(Self::SplitDown),
            "close" | "close_pane" => Some(Self::ClosePane),
            "focus_next" => Some(Self::FocusNext),
            "focus_prev" => Some(Self::FocusPrev),
            "focus_left" => Some(Self::FocusLeft),
            "focus_right" => Some(Self::FocusRight),
            "focus_up" => Some(Self::FocusUp),
            "focus_down" => Some(Self::FocusDown),
            "resize_left" => Some(Self::ResizeLeft),
            "resize_right" => Some(Self::ResizeRight),
            "resize_up" => Some(Self::ResizeUp),
            "resize_down" => Some(Self::ResizeDown),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_named_commands_parse() {
        let cases = [
            ("split_vertical", Command::SplitVertical),
            ("split_horizontal", Command::SplitHorizontal),
            ("split", Command::SplitVertical),
            ("split_left", Command::SplitLeft),
            ("split_right", Command::SplitRight),
            ("split_up", Command::SplitUp),
            ("split_down", Command::SplitDown),
            ("close", Command::ClosePane),
            ("close_pane", Command::ClosePane),
            ("focus_next", Command::FocusNext),
            ("focus_prev", Command::FocusPrev),
            ("focus_left", Command::FocusLeft),
            ("focus_right", Command::FocusRight),
            ("focus_up", Command::FocusUp),
            ("focus_down", Command::FocusDown),
            ("resize_left", Command::ResizeLeft),
            ("resize_right", Command::ResizeRight),
            ("resize_up", Command::ResizeUp),
            ("resize_down", Command::ResizeDown),
            ("quit", Command::Quit),
        ];
        for (name, expected) in cases {
            assert_eq!(
                Command::from_name(name),
                Some(expected),
                "failed for '{name}'"
            );
        }
    }

    #[test]
    fn unknown_command_returns_none() {
        assert!(Command::from_name("").is_none());
        assert!(Command::from_name("QUIT").is_none());
        assert!(Command::from_name("noop").is_none());
    }
}
