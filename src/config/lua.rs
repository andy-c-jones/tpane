use std::path::PathBuf;

use anyhow::{Context, Result};
use mlua::prelude::*;

use crate::config::defaults::DEFAULT_CONFIG;
use crate::core::commands::Command;
use crate::core::keymap::{KeyChord, KeyMap};

/// Resolved configuration after loading main.lua.
pub struct LuaConfig {
    pub keymap: KeyMap,
    /// Commands to run at startup (from tpane.on_startup).
    #[allow(dead_code)]
    pub startup_commands: Vec<Command>,
    /// Show keybinding cheatsheet when prefix key is active.
    pub show_cheatsheet: bool,
    _lua: Lua,
}

impl LuaConfig {
    /// Return the platform-appropriate config directory path.
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

    pub fn config_file() -> PathBuf {
        Self::config_dir().join("main.lua")
    }

    /// Ensure the config directory and default file exist.
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

    /// Load and evaluate main.lua, returning the resolved config.
    pub fn load() -> Result<Self> {
        Self::init_if_missing()?;
        let source = std::fs::read_to_string(Self::config_file())
            .context("reading main.lua")?;
        Self::load_from_source(&source)
    }

    /// Load config from a raw Lua source string (used in tests and for future embedding).
    pub fn load_from_source(source: &str) -> Result<Self> {
        // mlua 0.11: LuaError's inner source field is Arc<dyn Error> (no Send+Sync), so it no
        // longer satisfies anyhow's From<E: Send+Sync> bound.  Convert explicitly via format.
        fn lua_err(e: mlua::Error) -> anyhow::Error { anyhow::anyhow!("{e}") }

        let lua = Lua::new();
        let mut keymap = KeyMap::default();

        // Build the `tpane` table that Lua scripts interact with.
        // Bindings are collected into a Rust-side Vec and applied after execution.
        let bindings: std::sync::Arc<parking_lot::Mutex<Vec<(String, String)>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

        {
            let bindings_ref = bindings.clone();

            let tpane_table = lua.create_table().map_err(lua_err)?;

            // tpane.bind(chord, command_name)
            let bind_fn = {
                let b = bindings_ref.clone();
                lua.create_function(move |_, (chord, cmd): (String, String)| {
                    b.lock().push((chord, cmd));
                    Ok(())
                }).map_err(lua_err)?
            };
            tpane_table.set("bind", bind_fn).map_err(lua_err)?;

            // tpane.bind_direct(chord, command_name) — fires without the prefix key; holdable.
            let bind_direct_fn = {
                let b = bindings_ref.clone();
                lua.create_function(move |_, (chord, cmd): (String, String)| {
                    b.lock().push((format!("__direct__{}", chord), cmd));
                    Ok(())
                }).map_err(lua_err)?
            };
            tpane_table.set("bind_direct", bind_direct_fn).map_err(lua_err)?;

            // tpane.on_startup(fn) — accepted but startup logic is deferred via __startup__ keys
            let on_startup_fn = {
                lua.create_function(move |_, f: LuaFunction| {
                    // Call immediately so split helpers can record their commands
                    // mlua 0.11: Function::call takes one generic arg (return type only)
                    let _ = f.call::<()>(());
                    Ok(())
                }).map_err(lua_err)?
            };
            tpane_table.set("on_startup", on_startup_fn).map_err(lua_err)?;

            // Expose split helpers for use inside on_startup.
            for name in &[
                "split_vertical", "split_horizontal",
                "split_left", "split_right", "split_up", "split_down",
                "close", "focus_next", "focus_prev",
            ] {
                let n = *name;
                let b = bindings_ref.clone();
                let stub = lua.create_function(move |_, ()| {
                    b.lock().push((format!("__startup__{}", n), String::new()));
                    Ok(())
                }).map_err(lua_err)?;
                tpane_table.set(n, stub).map_err(lua_err)?;
            }

            // Default settings (can be overridden by user Lua code).
            tpane_table.set("show_cheatsheet", true).map_err(lua_err)?;

            lua.globals().set("tpane", tpane_table).map_err(lua_err)?;
        }

        // Execute the Lua source.
        lua.load(source).set_name("main.lua").exec().map_err(lua_err).context("executing lua source")?;

        // Read back settings from the tpane table.
        // mlua 0.11: Table::get takes one generic arg (value type only)
        let show_cheatsheet = lua.globals()
            .get::<LuaTable>("tpane")
            .ok()
            .and_then(|t| t.get::<bool>("show_cheatsheet").ok())
            .unwrap_or(true); // default: on

        // Apply collected bindings to the keymap; collect startup commands.
        let mut startup_commands: Vec<Command> = Vec::new();
        for (chord_str, cmd_name) in bindings.lock().iter() {
            if let Some(stripped) = chord_str.strip_prefix("__startup__") {
                if let Some(cmd) = Command::from_name(stripped) {
                    startup_commands.push(cmd);
                }
            } else if let Some(stripped) = chord_str.strip_prefix("__direct__") {
                if let Some(chord) = KeyChord::parse(stripped) {
                    if let Some(cmd) = Command::from_name(cmd_name) {
                        keymap.bind_direct(chord, cmd);
                    } else {
                        log::warn!("Unknown command '{}' in main.lua bind_direct call", cmd_name);
                    }
                } else {
                    log::warn!("Could not parse key chord '{}' in main.lua bind_direct call", stripped);
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

        Ok(LuaConfig { keymap, startup_commands, show_cheatsheet, _lua: lua })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::defaults::DEFAULT_CONFIG;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn ctrl_shift(c: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    // ── default config loads without errors ───────────────────────────────────

    #[test]
    fn default_config_loads_successfully() {
        let cfg = LuaConfig::load_from_source(DEFAULT_CONFIG).unwrap();
        assert!(cfg.startup_commands.is_empty(), "default config should have no startup commands");
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
        assert_eq!(cfg.startup_commands, vec![Command::SplitVertical, Command::SplitHorizontal]);
    }

    #[test]
    fn on_startup_empty_function_no_commands() {
        let src = r#"tpane.on_startup(function() end)"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert!(cfg.startup_commands.is_empty());
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
        assert_eq!(cfg.keymap.lookup_direct(&event), Some(&Command::ResizeRight));
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
            (KeyCode::Left,  Command::ResizeLeft),
            (KeyCode::Right, Command::ResizeRight),
            (KeyCode::Up,    Command::ResizeUp),
            (KeyCode::Down,  Command::ResizeDown),
        ];
        for (code, expected) in cases {
            let event = crossterm::event::KeyEvent::new(*code, alt_shift);
            assert_eq!(cfg.keymap.lookup_direct(&event), Some(expected),
                "missing direct binding for {code:?}");
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
        assert_eq!(cfg.startup_commands, vec![Command::SplitRight]);
    }

    #[test]
    fn on_startup_split_down_records_command() {
        let src = r#"
tpane.on_startup(function()
  tpane.split_down()
end)
"#;
        let cfg = LuaConfig::load_from_source(src).unwrap();
        assert_eq!(cfg.startup_commands, vec![Command::SplitDown]);
    }
}

