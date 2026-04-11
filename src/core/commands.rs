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
    Quit,
}

impl Command {
    /// Parse a command name string from Lua config into a Command.
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
            ("split_vertical",   Command::SplitVertical),
            ("split_horizontal", Command::SplitHorizontal),
            ("split",            Command::SplitVertical),
            ("split_left",       Command::SplitLeft),
            ("split_right",      Command::SplitRight),
            ("split_up",         Command::SplitUp),
            ("split_down",       Command::SplitDown),
            ("close",            Command::ClosePane),
            ("close_pane",       Command::ClosePane),
            ("focus_next",       Command::FocusNext),
            ("focus_prev",       Command::FocusPrev),
            ("quit",             Command::Quit),
        ];
        for (name, expected) in cases {
            assert_eq!(Command::from_name(name), Some(expected), "failed for '{name}'");
        }
    }

    #[test]
    fn unknown_command_returns_none() {
        assert!(Command::from_name("").is_none());
        assert!(Command::from_name("QUIT").is_none());
        assert!(Command::from_name("noop").is_none());
    }
}
