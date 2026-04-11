/// Embedded default Lua configuration written to ~/.config/tpane/main.lua on first run.
pub const DEFAULT_CONFIG: &str = r#"-- tpane configuration
-- This file is Lua. You can map keys to built-in commands or custom functions.
--
-- Prefix key: Ctrl+B (press first, then the command key)
--
-- Built-in commands:
--   "split_left"       - split active pane, new pane on the left
--   "split_right"      - split active pane, new pane on the right
--   "split_up"         - split active pane, new pane above
--   "split_down"       - split active pane, new pane below
--   "split_vertical"   - split the active pane vertically (alias for split_right)
--   "split_horizontal" - split the active pane horizontally (alias for split_down)
--   "close"            - close the active pane
--   "focus_next"       - move focus to next pane
--   "focus_prev"       - move focus to previous pane
--   "quit"             - exit tpane

-- Key bindings (applied after Ctrl+B prefix)
-- Format: tpane.bind("<modifiers+key>", "<command>")
tpane.bind("ctrl+left",  "split_left")
tpane.bind("ctrl+right", "split_right")
tpane.bind("ctrl+up",    "split_up")
tpane.bind("ctrl+down",  "split_down")
tpane.bind("left",  "focus_prev")
tpane.bind("right", "focus_next")
tpane.bind("up",    "focus_prev")
tpane.bind("down",  "focus_next")
tpane.bind("w", "close")
tpane.bind("q", "quit")

-- Startup layout (optional)
-- By default tpane opens with a single pane.
-- Uncomment and edit the block below to define a custom startup layout:
--
-- tpane.on_startup(function()
--   tpane.split_vertical()
--   tpane.split_horizontal()
-- end)
"#;
