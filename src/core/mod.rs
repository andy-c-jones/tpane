//! Core domain logic for tpane.
//!
//! These modules are intentionally platform-agnostic and focus on command
//! parsing, key mapping, pane layout, and text selection behavior.
//!
//! # Module map
//!
//! - [`commands`]: command enum + parser
//! - [`keymap`]: key-chord parsing and lookup
//! - [`layout`]: pane tree, geometry, and focus logic
//! - [`selection`]: pane-local selection representation

pub mod commands;
pub mod keymap;
pub mod layout;
pub mod selection;
