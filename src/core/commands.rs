/// All actions that can be dispatched in tpane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SplitVertical,
    SplitHorizontal,
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
