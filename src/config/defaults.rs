/// Embedded default Lua configuration written to ~/.config/tpane/main.lua on first run.
pub const DEFAULT_CONFIG: &str = r#"-- tpane configuration
-- This file is Lua. You can map keys to built-in commands or custom functions.
--
-- Built-in commands:
--   "split_vertical"   - split the active pane vertically (left | right)
--   "split_horizontal" - split the active pane horizontally (top / bottom)
--   "split"            - alias for split_vertical
--   "close"            - close the active pane
--   "focus_next"       - move focus to next pane
--   "focus_prev"       - move focus to previous pane
--   "quit"             - exit tpane

-- Key bindings
-- Format: tpane.bind("<modifiers+key>", "<command>" | function)
tpane.bind("ctrl+shift+t", "split_vertical")
tpane.bind("ctrl+shift+v", "split_vertical")
tpane.bind("ctrl+shift+h", "split_horizontal")
tpane.bind("ctrl+shift+w", "close")
tpane.bind("ctrl+shift+n", "focus_next")
tpane.bind("ctrl+shift+p", "focus_prev")
tpane.bind("ctrl+shift+q", "quit")

-- Startup layout (optional)
-- By default tpane opens with a single pane.
-- Uncomment and edit the block below to define a custom startup layout:
--
-- tpane.on_startup(function()
--   tpane.split_vertical()
--   tpane.split_horizontal()
-- end)
"#;
