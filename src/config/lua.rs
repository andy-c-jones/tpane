//! Lua-backed configuration loading and binding extraction.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use mlua::prelude::*;

use crate::config::defaults::DEFAULT_CONFIG;
use crate::core::commands::{Command, LayoutAction};
use crate::core::keymap::{KeyChord, KeyMap};

/// Resolved configuration after loading `main.lua`.
///
/// # Fields
///
/// - [`Self::keymap`]: resolved prefix/direct key bindings
/// - [`Self::startup_commands`]: startup layout steps captured from `tpane.on_startup`
/// - [`Self::named_layouts`]: named layout definitions from `tpane.define_layout`
/// - [`Self::show_cheatsheet`]: whether prefix-mode cheatsheet is shown
pub struct LuaConfig {
    /// Effective key map consumed by [`crate::app::App`].
    pub keymap: KeyMap,
    /// Layout steps to run at startup (from `tpane.on_startup { ... }`).
    pub startup_commands: Vec<LayoutAction>,
    /// Named layouts indexed by number (from `tpane.define_layout(N, ...)`).
    ///
    /// Each layout is a sequence of [`LayoutAction`]s that [`crate::app::App`]
    /// can apply to reset and rebuild the pane tree.
    pub named_layouts: HashMap<u8, Vec<LayoutAction>>,
    /// Show keybinding cheatsheet when prefix key is active.
    ///
    /// This toggles prefix-mode UI hints in [`crate::platform::renderer`].
    pub show_cheatsheet: bool,
    _lua: Lua,
}

impl LuaConfig {
    /// Return the platform-appropriate config directory path.
    ///
    /// This resolves to `$XDG_CONFIG_HOME/tpane` (or `~/.config/tpane`) on
    /// Unix-like systems and `%APPDATA%\\tpane` on Windows.
    pub fn config_dir() -> PathBuf {
        // Linux/macOS: ~/.config/tpane
        // Windows:     %APPDATA%\tpane
        #[cfg(not(target_os = "windows"))]
        let base = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
        #[cfg(target_os = "windows")]
        let base = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));

        base.join("tpane")
    }

    /// Return the path to the main configuration file (`main.lua`).
    ///
    /// # Examples
    ///
    /// ```text
    /// let path = LuaConfig::config_file();
    /// // ~/.config/tpane/main.lua (unix-like)
    /// ```
    pub fn config_file() -> PathBuf {
        Self::config_dir().join("main.lua")
    }

    /// Ensure the config directory and default file exist.
    ///
    /// If the file is missing, this writes [`DEFAULT_CONFIG`].
    ///
    /// # Errors
    ///
    /// Returns I/O errors when directory creation or file writes fail.
    pub fn init_if_missing() -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating config dir {}", dir.display()))?;
        }
        let file = Self::config_file();
        if !file.exists() {
            std::fs::write(&file, DEFAULT_CONFIG)
                .with_context(|| format!("writing default config to {}", file.display()))?;
        }
        Ok(())
    }

    /// Load and evaluate `main.lua`, returning the resolved config.
    ///
    /// # Errors
    ///
    /// Returns an error when file I/O fails or Lua evaluation reports a syntax
    /// or runtime error.
    pub fn load() -> Result<Self> {
        Self::init_if_missing()?;
        let source = std::fs::read_to_string(Self::config_file()).context("reading main.lua")?;
        Self::load_from_source(&source)
    }

    /// Load config from a raw Lua source string.
    ///
    /// This is used by tests and can also be used by future embedding
    /// integrations that provide in-memory config content.
    ///
    /// # Behavior
    ///
    /// Unknown commands and invalid key chords are ignored, matching runtime
    /// behavior for permissive user configuration.
    pub fn load_from_source(source: &str) -> Result<Self> {
        // mlua 0.11: LuaError's inner source field is Arc<dyn Error> (no Send+Sync), so it no
        // longer satisfies anyhow's From<E: Send+Sync> bound.  Convert explicitly via format.
        fn lua_err(e: mlua::Error) -> anyhow::Error {
            anyhow::anyhow!("{e}")
        }

        let lua = Lua::new();
        let mut keymap = KeyMap::default();

        // ── Shared recording state ────────────────────────────────────────────

        // Flat list of (chord_str, cmd_name) for keybindings (prefix + direct).
        let bindings: std::sync::Arc<parking_lot::Mutex<Vec<(String, String)>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

        // Typed action recorders.  Split helpers and tpane.run() write into
        // whichever Vec is "current".  Default target = startup_actions (for
        // on_startup).  define_layout(N, fn) swaps the target to a temporary
        // layout-specific Vec for the duration of the callback.
        let startup_actions: std::sync::Arc<parking_lot::Mutex<Vec<LayoutAction>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

        // While inside a define_layout callback this holds the live Vec being
        // built for layout N.  None when we're outside any layout definition.
        let layout_recording: std::sync::Arc<parking_lot::Mutex<Option<Vec<LayoutAction>>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(None));

        // Named layouts collected after define_layout callbacks complete.
        let named_layouts_store: std::sync::Arc<
            parking_lot::Mutex<HashMap<u8, Vec<LayoutAction>>>,
        > = std::sync::Arc::new(parking_lot::Mutex::new(HashMap::new()));

        {
            let bindings_ref = bindings.clone();
            let tpane_table = lua.create_table().map_err(lua_err)?;

            // ── tpane.bind / tpane.bind_direct ───────────────────────────────

            let bind_fn = {
                let b = bindings_ref.clone();
                lua.create_function(move |_, (chord, cmd): (String, String)| {
                    b.lock().push((chord, cmd));
                    Ok(())
                })
                .map_err(lua_err)?
            };
            tpane_table.set("bind", bind_fn).map_err(lua_err)?;

            let bind_direct_fn = {
                let b = bindings_ref.clone();
                lua.create_function(move |_, (chord, cmd): (String, String)| {
                    b.lock().push((format!("__direct__{}", chord), cmd));
                    Ok(())
                })
                .map_err(lua_err)?
            };
            tpane_table
                .set("bind_direct", bind_direct_fn)
                .map_err(lua_err)?;

            // ── tpane.run(program) ────────────────────────────────────────────
            // Sends `program + "\n"` to the currently-active pane's shell.
            // Works both inside on_startup and inside define_layout.
            {
                let sa = startup_actions.clone();
                let lr = layout_recording.clone();
                let run_fn = lua
                    .create_function(move |_, program: String| {
                        let action = LayoutAction::RunInPane(program);
                        let mut recording = lr.lock();
                        if let Some(steps) = recording.as_mut() {
                            steps.push(action);
                        } else {
                            sa.lock().push(action);
                        }
                        Ok(())
                    })
                    .map_err(lua_err)?;
                tpane_table.set("run", run_fn).map_err(lua_err)?;
            }

            // ── tpane.on_startup(fn) ──────────────────────────────────────────
            // Accepts a function and calls it immediately so split helpers and
            // tpane.run() can record their actions into startup_actions.
            {
                let on_startup_fn = lua
                    .create_function(move |_, f: LuaFunction| {
                        // mlua 0.11: Function::call takes one generic arg (return type only)
                        let _ = f.call::<()>(());
                        Ok(())
                    })
                    .map_err(lua_err)?;
                tpane_table
                    .set("on_startup", on_startup_fn)
                    .map_err(lua_err)?;
            }

            // ── tpane.define_layout(n, fn) ────────────────────────────────────
            // Defines named layout N.  During the callback, split helpers and
            // tpane.run() record into a temporary Vec that is stored under N.
            // Also auto-binds Ctrl+N → "load_layout_N" (can be overridden with
            // an explicit tpane.bind("ctrl+N", ...) after this call).
            {
                let lr = layout_recording.clone();
                let nls = named_layouts_store.clone();
                let b = bindings_ref.clone();
                let define_layout_fn = lua
                    .create_function(move |_, (n, f): (u8, LuaFunction)| {
                        *lr.lock() = Some(Vec::new());
                        let _ = f.call::<()>(());
                        if let Some(steps) = lr.lock().take() {
                            nls.lock().insert(n, steps);
                        }
                        // Auto-bind Ctrl+N → load_layout_N.
                        b.lock()
                            .push((format!("ctrl+{}", n), format!("load_layout_{}", n)));
                        Ok(())
                    })
                    .map_err(lua_err)?;
                tpane_table
                    .set("define_layout", define_layout_fn)
                    .map_err(lua_err)?;
            }

            // ── Split / focus helpers ─────────────────────────────────────────
            // Each helper accepts an optional ratio and records a LayoutAction::Split
            // into whichever Vec is currently active (define_layout or on_startup).
            for name in &[
                "split_vertical",
                "split_horizontal",
                "split_left",
                "split_right",
                "split_up",
                "split_down",
                "close",
                "focus_next",
                "focus_prev",
                "focus_left",
                "focus_right",
                "focus_up",
                "focus_down",
            ] {
                let n = *name;
                let sa = startup_actions.clone();
                let lr = layout_recording.clone();
                let stub = lua
                    .create_function(move |_, ratio: Option<f64>| {
                        if let Some(cmd) = Command::from_name(n) {
                            let action = LayoutAction::Split { cmd, ratio };
                            let mut recording = lr.lock();
                            if let Some(steps) = recording.as_mut() {
                                steps.push(action);
                            } else {
                                sa.lock().push(action);
                            }
                        }
                        Ok(())
                    })
                    .map_err(lua_err)?;
                tpane_table.set(n, stub).map_err(lua_err)?;
            }

            // Default settings (can be overridden by user Lua code).
            tpane_table.set("show_cheatsheet", true).map_err(lua_err)?;

            lua.globals().set("tpane", tpane_table).map_err(lua_err)?;
        }

        // Execute the Lua source.
        lua.load(source)
            .set_name("main.lua")
            .exec()
            .map_err(lua_err)
            .context("executing lua source")?;

        // Read back settings from the tpane table.
        let show_cheatsheet = lua
            .globals()
            .get::<LuaTable>("tpane")
            .ok()
            .and_then(|t| t.get::<bool>("show_cheatsheet").ok())
            .unwrap_or(true);

        // Apply collected keybindings to the keymap.
        for (chord_str, cmd_name) in bindings.lock().iter() {
            if let Some(stripped) = chord_str.strip_prefix("__direct__") {
                if let Some(chord) = KeyChord::parse(stripped) {
                    if let Some(cmd) = Command::from_name(cmd_name) {
                        keymap.bind_direct(chord, cmd);
                    } else {
                        log::warn!(
                            "Unknown command '{}' in main.lua bind_direct call",
                            cmd_name
                        );
                    }
                } else {
                    log::warn!(
                        "Could not parse key chord '{}' in main.lua bind_direct call",
                        stripped
                    );
                }
            } else if let Some(chord) = KeyChord::parse(chord_str) {
                if let Some(cmd) = Command::from_name(cmd_name) {
                    keymap.bind(chord, cmd);
                } else {
                    log::warn!("Unknown command '{}' in main.lua bind call", cmd_name);
                }
            } else {
                log::warn!("Could not parse key chord '{}' in main.lua", chord_str);
            }
        }

        let startup_commands = std::mem::take(&mut *startup_actions.lock());
        let named_layouts = std::mem::take(&mut *named_layouts_store.lock());

        Ok(LuaConfig {
            keymap,
            startup_commands,
            named_layouts,
            show_cheatsheet,
            _lua: lua,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::defaults::DEFAULT_CONFIG;
    use crossterm::event::{KeyCode, KeyModifiers};

    // ── default config loads without errors ───────────────────────────────────

    #[test]
    fn default_config_loads_successfully() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        assert!(
            cfg.startup_commands.is_empty(),
            "default config should have no startup commands"
        );
    }

    #[test]
    fn default_config_has_split_right_binding() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        let cmd = cfg.keymap.lookup_prefix(&event);
        assert_eq!(cmd, Some(&Command::SplitRight));
    }

    #[test]
    fn default_config_has_close_pane_binding() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty());
        let cmd = cfg.keymap.lookup_prefix(&event);
        assert_eq!(cmd, Some(&Command::ClosePane));
    }

    // ── custom bindings ───────────────────────────────────────────────────────

    #[test]
    fn custom_bind_overrides_default() {
        let src = r#"tpane.bind("w", "quit")"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty());
        let cmd = cfg.keymap.lookup_prefix(&event);
        assert_eq!(cmd, Some(&Command::Quit));
    }

    #[test]
    fn valid_bind_adds_to_keymap() {
        let src = r#"tpane.bind("x", "focus_next")"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty());
        assert_eq!(cfg.keymap.lookup_prefix(&event), Some(&Command::FocusNext));
    }

    #[test]
    fn unknown_command_is_silently_ignored() {
        // "badcmd" is not a known Command name — should not crash, just warn
        let src = r#"tpane.bind("ctrl+shift+z", "badcmd")"#;
        let result = LuaConfig::load_from_source(src);
        assert!(result.is_ok());
    }

    #[test]
    fn bad_chord_is_silently_ignored() {
        let src = r#"tpane.bind("not+a+chord", "quit")"#;
        let result = LuaConfig::load_from_source(src);
        assert!(result.is_ok());
    }

    #[test]
    fn duplicate_bind_last_wins() {
        let src = r#"
tpane.bind("d", "focus_next")
tpane.bind("d", "focus_prev")
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty());
        assert_eq!(cfg.keymap.lookup_prefix(&event), Some(&Command::FocusPrev));
    }

    // ── on_startup ────────────────────────────────────────────────────────────

    #[test]
    fn on_startup_records_startup_commands() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_vertical()
  tpane.split_horizontal()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(
            cfg.startup_commands,
            vec![
                LayoutAction::Split {
                    cmd: Command::SplitVertical,
                    ratio: None
                },
                LayoutAction::Split {
                    cmd: Command::SplitHorizontal,
                    ratio: None
                },
            ]
        );
    }

    #[test]
    fn on_startup_empty_function_no_commands() {
        let src = r#"tpane.on_startup(function() end)"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert!(cfg.startup_commands.is_empty());
    }

    #[test]
    fn on_startup_run_records_run_in_pane() {
        let src = r#"
tpane.on_startup(function()
  tpane.run("nvim .")
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(
            cfg.startup_commands,
            vec![LayoutAction::RunInPane("nvim .".to_string())]
        );
    }

    // ── error handling ────────────────────────────────────────────────────────

    #[test]
    fn lua_syntax_error_returns_err() {
        let src = r#"this is not valid lua !!!"#;
        let result = LuaConfig::load_from_source(src);
        assert!(result.is_err());
    }

    #[test]
    fn empty_source_loads_successfully() {
        let cfg = LuaConfig::load_from_source("").unwrap();
        // No custom bindings; default bindings from KeyMap::default() still apply.
        assert!(cfg.startup_commands.is_empty());
    }

    // ── show_cheatsheet config ───────────────────────────────────────────────

    #[test]
    fn default_config_enables_cheatsheet() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        assert!(cfg.show_cheatsheet);
    }

    #[test]
    fn cheatsheet_defaults_to_true_when_not_set() {
        let cfg = LuaConfig::load_from_source("").unwrap();
        assert!(cfg.show_cheatsheet);
    }

    #[test]
    fn cheatsheet_can_be_disabled_via_lua() {
        let cfg = LuaConfig::load_from_source("tpane.show_cheatsheet = false").unwrap();
        assert!(!cfg.show_cheatsheet);
    }

    // ── bind_direct ───────────────────────────────────────────────────────────

    #[test]
    fn bind_direct_adds_direct_binding() {
        let src = r#"tpane.bind_direct("alt+r", "resize_right")"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT);
        assert_eq!(
            cfg.keymap.lookup_direct(&event),
            Some(&Command::ResizeRight)
        );
    }

    #[test]
    fn bind_direct_does_not_add_prefix_binding() {
        let src = r#"tpane.bind_direct("alt+r", "resize_right")"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT);
        assert!(cfg.keymap.lookup_prefix(&event).is_none());
    }

    #[test]
    fn bind_direct_unknown_command_is_silently_ignored() {
        let src = r#"tpane.bind_direct("alt+r", "not_a_command")"#;
        let result = LuaConfig::load_from_source(src);
        assert!(result.is_ok());
    }

    #[test]
    fn bind_direct_bad_chord_is_silently_ignored() {
        let src = r#"tpane.bind_direct("not+a+chord", "resize_right")"#;
        let result = LuaConfig::load_from_source(src);
        assert!(result.is_ok());
    }

    #[test]
    fn default_config_has_resize_direct_bindings() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
        let cases: &[(KeyCode, Command)] = &[
            (KeyCode::Left, Command::ResizeLeft),
            (KeyCode::Right, Command::ResizeRight),
            (KeyCode::Up, Command::ResizeUp),
            (KeyCode::Down, Command::ResizeDown),
        ];
        for (code, expected) in cases {
            let event = crossterm::event::KeyEvent::new(*code, alt_shift);
            assert_eq!(
                cfg.keymap.lookup_direct(&event),
                Some(expected),
                "missing direct binding for {code:?}"
            );
        }
    }

    // ── on_startup with split_right / split_down ──────────────────────────────

    #[test]
    fn on_startup_split_right_records_command() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_right()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(
            cfg.startup_commands,
            vec![LayoutAction::Split {
                cmd: Command::SplitRight,
                ratio: None
            }]
        );
    }

    #[test]
    fn on_startup_split_down_records_command() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_down()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(
            cfg.startup_commands,
            vec![LayoutAction::Split {
                cmd: Command::SplitDown,
                ratio: None
            }]
        );
    }

    // ── on_startup with explicit ratio ───────────────────────────────────────

    #[test]
    fn on_startup_split_right_with_ratio_records_ratio() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_right(0.3)
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        match &cfg.startup_commands[0] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitRight);
                assert!((ratio.unwrap() - 0.3).abs() < 1e-9);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn on_startup_split_down_with_ratio_records_ratio() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_down(0.6)
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        match &cfg.startup_commands[0] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitDown);
                assert!((ratio.unwrap() - 0.6).abs() < 1e-9);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn on_startup_multiple_splits_with_mixed_ratios() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_right(0.7)
  tpane.split_down()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(cfg.startup_commands.len(), 2);
        match &cfg.startup_commands[0] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitRight);
                assert!((ratio.unwrap() - 0.7).abs() < 1e-9);
            }
            other => panic!("expected Split, got {other:?}"),
        }
        match &cfg.startup_commands[1] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitDown);
                assert!(ratio.is_none());
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    // ── define_layout ────────────────────────────────────────────────────────

    #[test]
    fn define_layout_records_named_layout() {
        let src = r#"
tpane.define_layout(1, function()
  tpane.split_right(0.6)
  tpane.split_down()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert!(cfg.startup_commands.is_empty());
        let layout = cfg.named_layouts.get(&1).expect("layout 1 should exist");
        assert_eq!(layout.len(), 2);
        match &layout[0] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitRight);
                assert!((ratio.unwrap() - 0.6).abs() < 1e-9);
            }
            other => panic!("expected Split, got {other:?}"),
        }
        match &layout[1] {
            LayoutAction::Split { cmd, ratio } => {
                assert_eq!(*cmd, Command::SplitDown);
                assert!(ratio.is_none());
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn define_layout_with_run_records_run_in_pane() {
        let src = r#"
tpane.define_layout(1, function()
  tpane.run("nvim .")
  tpane.split_right(0.6)
  tpane.run("lazygit")
  tpane.split_down()
  tpane.run("copilot")
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let layout = cfg.named_layouts.get(&1).unwrap();
        assert_eq!(layout.len(), 5);
        assert_eq!(layout[0], LayoutAction::RunInPane("nvim .".to_string()));
        assert_eq!(layout[2], LayoutAction::RunInPane("lazygit".to_string()));
        assert_eq!(layout[4], LayoutAction::RunInPane("copilot".to_string()));
    }

    #[test]
    fn define_layout_auto_binds_ctrl_n() {
        let src = r#"
tpane.define_layout(1, function()
  tpane.split_right()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL);
        assert_eq!(
            cfg.keymap.lookup_prefix(&event),
            Some(&Command::LoadLayout(1))
        );
    }

    #[test]
    fn define_layout_auto_bind_overridable() {
        // An explicit tpane.bind after define_layout should override the auto-bind.
        let src = r#"
tpane.define_layout(1, function() end)
tpane.bind("ctrl+1", "quit")
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        let event = crossterm::event::KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL);
        assert_eq!(cfg.keymap.lookup_prefix(&event), Some(&Command::Quit));
    }

    #[test]
    fn define_layout_does_not_pollute_startup_commands() {
        let src = r#"
tpane.define_layout(2, function()
  tpane.split_right()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert!(cfg.startup_commands.is_empty());
    }

    #[test]
    fn multiple_define_layouts_are_independent() {
        let src = r#"
tpane.define_layout(1, function()
  tpane.split_right()
end)
tpane.define_layout(2, function()
  tpane.split_down()
  tpane.split_right()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(cfg.named_layouts.get(&1).unwrap().len(), 1);
        assert_eq!(cfg.named_layouts.get(&2).unwrap().len(), 2);
    }
}
