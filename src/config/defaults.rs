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
--   "resize_left"      - grow the active pane to the left
--   "resize_right"     - grow the active pane to the right
--   "resize_up"        - grow the active pane upward
--   "resize_down"      - grow the active pane downward
--   "quit"             - exit tpane

-- Key bindings (applied after Ctrl+B prefix)
-- Format: tpane.bind("<modifiers+key>", "<command>")
tpane.bind("ctrl+left",  "split_left")
tpane.bind("ctrl+right", "split_right")
tpane.bind("ctrl+up",    "split_up")
tpane.bind("ctrl+down",  "split_down")
tpane.bind("left",  "focus_left")
tpane.bind("right", "focus_right")
tpane.bind("up",    "focus_up")
tpane.bind("down",  "focus_down")
tpane.bind("w", "close")
tpane.bind("q", "quit")

-- Resize bindings (direct keys — no prefix needed; hold to move edges slowly)
-- Format: tpane.bind_direct("<modifiers+key>", "<command>")
tpane.bind_direct("alt+shift+left",  "resize_left")
tpane.bind_direct("alt+shift+right", "resize_right")
tpane.bind_direct("alt+shift+up",    "resize_up")
tpane.bind_direct("alt+shift+down",  "resize_down")

-- You can also drag a divider with the mouse: left-click and drag the line
-- between two panes to resize them interactively.

-- ── Startup layouts ────────────────────────────────────────────────────────
-- By default tpane opens with a single pane.
-- Uncomment ONE of the blocks below to use a preset layout at startup.

-- 2-column layout (two equal vertical panes side by side):
--
-- tpane.on_startup(function()
--   tpane.split_right()
-- end)

-- 3-column layout (25% | 50% | 25%):
-- After splitting, adjust the ratios in ~/.config/tpane/main.lua or drag
-- the dividers with the mouse to fine-tune.
--
-- tpane.on_startup(function()
--   tpane.split_right()   -- creates left (50%) | right (50%)
--   tpane.split_right()   -- splits right half: center (50%) | far-right (50%)
-- end)

-- 3-pane layout (one wide left column + two stacked rows on the right):
--
-- tpane.on_startup(function()
--   tpane.split_right()   -- left | right
--   tpane.split_down()    -- right is split into top-right | bottom-right
-- end)

-- Settings
-- Show keybinding cheatsheet when prefix key (Ctrl+B) is pressed.
-- Set to false to disable.
tpane.show_cheatsheet = true
"#;
