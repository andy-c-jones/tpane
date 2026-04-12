//! Configuration loading and defaults.
//!
//! The configuration system is Lua-based and supports startup commands,
//! keybindings, and UI toggles.
//!
//! - [`defaults`]: embedded `main.lua` template
//! - [`lua`]: runtime loading/parsing and command extraction

pub mod defaults;
pub mod lua;
